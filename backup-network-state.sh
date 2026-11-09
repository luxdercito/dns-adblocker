#!/usr/bin/env bash
# Backup current network and DNS state
BACKUP_DIR="$HOME/dns-adblocker/network-backup-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$BACKUP_DIR"

echo "Backing up network state to $BACKUP_DIR..."

resolvectl status > "$BACKUP_DIR/resolvectl-status.txt" 2>&1
cat /etc/resolv.conf > "$BACKUP_DIR/resolv.conf" 2>&1
[ -f /etc/systemd/resolved.conf ] && cp /etc/systemd/resolved.conf "$BACKUP_DIR/resolved.conf"
[ -d /etc/systemd/resolved.conf.d ] && cp -r /etc/systemd/resolved.conf.d "$BACKUP_DIR/resolved.conf.d"
sudo nft list ruleset > "$BACKUP_DIR/nftables-ruleset.txt" 2>&1 || echo "nftables not available" > "$BACKUP_DIR/nftables-ruleset.txt"
sudo iptables-save > "$BACKUP_DIR/iptables-rules.v4" 2>&1
nmcli con show > "$BACKUP_DIR/nmcli-connections.txt" 2>&1
ip addr show > "$BACKUP_DIR/ip-addr.txt" 2>&1
ip route show > "$BACKUP_DIR/ip-route.txt" 2>&1

echo "Backup completed successfully in $BACKUP_DIR"
