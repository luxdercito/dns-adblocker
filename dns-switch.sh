#!/usr/bin/env bash
set -euo pipefail

STATE_DIR="$HOME/.config/dns-adblocker/state"
mkdir -p "$STATE_DIR"
SNAPSHOT="$STATE_DIR/dns-snapshot.json"
ADBLOCKER_PORT="${ADBLOCKER_PORT:-53}"
ADBLOCKER_ADDR="127.0.0.1"

backup_state() {
    echo "Saving current DNS state..."
    local resolv_conf
    resolv_conf=$(cat /etc/resolv.conf 2>/dev/null || echo "")
    local resolvectl_status
    resolvectl_status=$(resolvectl status 2>/dev/null || echo "")
    local resolved_conf
    resolved_conf=$(cat /etc/systemd/resolved.conf 2>/dev/null || echo "")
    local nm_connections
    nm_connections=$(nmcli -t -f NAME,DEVICE connection show --active 2>/dev/null || echo "")
    
    # Save to JSON-like format
    cat > "$SNAPSHOT" << EOF
{
  "timestamp": "$(date -Iseconds)",
  "resolv_conf": $(printf '%s' "$resolv_conf" | python3 -c "import sys,json; print(json.dumps(sys.stdin.read()))"),
  "resolvectl_status": $(printf '%s' "$resolvectl_status" | python3 -c "import sys,json; print(json.dumps(sys.stdin.read()))"),
  "resolved_conf": $(printf '%s' "$resolved_conf" | python3 -c "import sys,json; print(json.dumps(sys.stdin.read()))"),
  "nm_connections": $(printf '%s' "$nm_connections" | python3 -c "import sys,json; print(json.dumps(sys.stdin.read()))")
}
EOF
    echo "Snapshot saved to $SNAPSHOT"
}

restore_state() {
    if [ ! -f "$SNAPSHOT" ]; then
        echo "No snapshot found at $SNAPSHOT"
        return 1
    fi
    echo "Restoring DNS state from snapshot..."
    
    # Restore /etc/resolv.conf
    local resolv_conf
    resolv_conf=$(python3 -c "import json; print(json.load(open('$SNAPSHOT'))['resolv_conf'])")
    if [ -n "$resolv_conf" ]; then
        echo "$resolv_conf" | sudo -A tee /etc/resolv.conf > /dev/null
        echo "Restored /etc/resolv.conf"
    fi
    
    # Restore resolved.conf if we had a backup
    local resolved_conf
    resolved_conf=$(python3 -c "import json; print(json.load(open('$SNAPSHOT')).get('resolved_conf',''))")
    if [ -n "$resolved_conf" ]; then
        echo "$resolved_conf" | sudo -A tee /etc/systemd/resolved.conf > /dev/null
        echo "Restored /etc/systemd/resolved.conf"
    fi
    
    sudo -A systemctl restart systemd-resolved 2>/dev/null || true
    echo "DNS state restored."
}

apply_adblocker() {
    echo "Configuring systemd-resolved to use adblocker at $ADBLOCKER_ADDR:$ADBLOCKER_PORT..."
    
    # Configure resolved.conf
    sudo -A tee /etc/systemd/resolved.conf > /dev/null << EOF
[Resolve]
DNS=$ADBLOCKER_ADDR
FallbackDNS=
DNSStubListener=no
EOF
    
    # Update /etc/resolv.conf
    sudo -A tee /etc/resolv.conf > /dev/null << EOF
nameserver $ADBLOCKER_ADDR
EOF
    
    sudo -A systemctl restart systemd-resolved 2>/dev/null || true
    echo "DNS configured to use adblocker."
}

start_adblocker() {
    local binary="/home/luxder/dns-adblocker/target/release/dns-adblocker"
    if [ ! -f "$binary" ]; then
        echo "Binary not found at $binary"
        return 1
    fi
    
    # Kill any existing instance
    sudo -A pkill -f dns-adblocker 2>/dev/null || true
    sleep 1
    
    # Start as systemd service if available
    if systemctl list-unit-files dns-adblocker.service >/dev/null 2>&1; then
        sudo -A systemctl start dns-adblocker
        echo "Started via systemd service"
    else
        sudo -A "$binary" &
        echo "Started binary directly"
    fi
    sleep 2
}

stop_adblocker() {
    sudo -A pkill -f dns-adblocker 2>/dev/null || true
    sudo -A systemctl stop dns-adblocker 2>/dev/null || true
    sleep 1
    echo "Adblocker stopped."
}

case "${1:-}" in
    push)
        backup_state
        start_adblocker
        apply_adblocker
        echo ""
        echo "Adblocker is now active. DNS -> $ADBLOCKER_ADDR:$ADBLOCKER_PORT"
        ;;
    pop)
        stop_adblocker
        restore_state
        echo ""
        echo "DNS restored to original state."
        ;;
    status)
        echo "=== Adblocker process ==="
        ps aux | grep '[d]ns-adblocker' || echo "Not running"
        echo ""
        echo "=== Port 53 ==="
        ss -ulnp 2>/dev/null | grep ':53 ' || echo "Not listening"
        echo ""
        echo "=== resolv.conf ==="
        cat /etc/resolv.conf
        echo ""
        echo "=== Snapshot ==="
        [ -f "$SNAPSHOT" ] && echo "Exists: $SNAPSHOT ($(date -r "$SNAPSHOT" -Iseconds))" || echo "No snapshot"
        ;;
    *)
        echo "Usage: $0 {push|pop|status}"
        echo "  push   - Save current DNS state, start adblocker, configure DNS"
        echo "  pop    - Stop adblocker, restore original DNS state"
        echo "  status - Show current state"
        exit 1
        ;;
esac
