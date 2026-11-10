# DNS AdBlocker

Un servidor DNS de nivel profesional que bloquea anuncios, trackers y malware usando el motor de filtrado de Brave.

## Características

- 🛡️ **Bloqueo DNS** — Resuelve dominios de publicidad y los redirige a `0.0.0.0`
- ⚡ **Async 100%** — Todo el código usa Tokio async/await, sin bloqueos del reactor
- 🔧 **Configurable** — Puerto, dirección bind, upstreams y listas editables desde JSON
- 🌐 **Soporte LAN** — Cambia `bind_addr` a `"0.0.0.0"` para atender toda la red
- 🔄 **Auto-refresh** — Actualiza las listas de filtros cada 24 horas automáticamente
- 📦 **Motor Brave** — Utiliza `adblock-rust` (el mismo que usa Brave Browser)

## Arquitectura

```
┌─────────────┐     UDP DNS      ┌──────────────────┐     Worker      ┌──────────────┐
│  Cliente    │◄────────────────►│  DNS Server      │◄──────────────►│  Engine      │
│  (Browser)  │   query          │  (Tokio UdpSocket)│   is_blocked   │  (Filtros)   │
└─────────────┘                  └──────────────────┘                └──────────────┘
                                       │                                      │
                                       │ blocked?                             │
                                       ▼                                      ▼
                              ┌──────────────────┐               ┌──────────────────┐
                              │ 0.0.0.0 response │               │ EasyList         │
                              │                  │               │ EasyPrivacy      │
                              └──────────────────┘               │ Fanboy* lists    │
                                       │                          └──────────────────┘
                                       │ not blocked
                                       ▼
                              ┌──────────────────┐
                              │ Upstream Resolver│
                              │ (1.1.1.1, etc.)  │
                              └──────────────────┘
```

## Instalación

### Requisitos

- Linux (probado en Ubuntu/Debian/Mint)
- Rust toolchain (`rustup`)
- Permisos root o `CAP_NET_BIND_SERVICE` para escuchar en puerto 53

### Desde source

```bash
git clone https://github.com/luxdercito/dns-adblocker.git
cd dns-adblocker
cargo build --release
sudo cp target/release/dns-adblocker /usr/local/bin/
```

### Desde binario precompilado

```bash
# Enlaza al binario release
sudo ln -sf /home/luxder/dns-adblocker/target/release/dns-adblocker /usr/local/bin/dns-adblocker
```

## Configuración

El archivo de configuración se encuentra en:
```
~/.config/dns-adblocker/config.json
```

```json
{
  "bind_port": 53,
  "bind_addr": "127.0.0.1",
  "upstream_dns": [
    "1.1.1.1:53",
    "1.0.0.1:53",
    "9.9.9.9:53",
    "8.8.8.8:53"
  ],
  "filter_lists_dir": "/home/luxder/.config/dns-adblocker/filter_lists",
  "blocked_domain_ip": "0.0.0.0",
  "refresh_hours": 24
}
```

### Parámetros

| Campo | Valor default | Descripción |
|-------|---------------|-------------|
| `bind_port` | `53` | Puerto donde escucha el servidor DNS |
| `bind_addr` | `127.0.0.1` | Dirección de enlace. Usa `0.0.0.0` para LAN |
| `upstream_dns` | Cloudflare + Google | Resolvedores upstream para reenviar consultas legítimas |
| `filter_lists_dir` | `~/.config/dns-adblocker/filter_lists` | Directorio de listas de filtros descargados |
| `blocked_domain_ip` | `0.0.0.0` | IP a la que se redirigen los dominios bloqueados |
| `refresh_hours` | `24` | Intervalo entre actualizaciones de filtros |

## Uso como servicio systemd

```bash
sudo tee /etc/systemd/system/dns-adblocker.service > /dev/null << 'EOF'
[Unit]
Description=DNS AdBlocker using Brave's adblock-rust engine
After=network.target

[Service]
Type=simple
User=root
ExecStart=/usr/local/bin/dns-adblocker
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable --now dns-adblocker
```

### Verificar estado

```bash
systemctl status dns-adblocker
journalctl -u dns-adblocker -f
```

## Configurar clientes para usar el DNS

### En el equipo local (Linux con systemd-resolved)

```bash
sudo mkdir -p /etc/systemd/resolved.conf.d
sudo tee /etc/systemd/resolved.conf.d/dns-adblocker.conf > /dev/null << 'EOF'
[Resolve]
DNS=127.0.0.1
FallbackDNS=1.1.1.1
EOF
sudo systemctl restart systemd-resolved
```

### Para LAN (otras máquinas de tu red)

1. Cambia `bind_addr` a `"0.0.0.0"` en `config.json`
2. Reinicia el servicio
3. Configura los clientes para usar la IP del servidor como DNS

```bash
# Ejemplo para el router
# DNS primary: 192.168.1.14 (IP del servidor)
```

## Listas de filtros incluidas

| Lista | Tamaño aprox. | Reglas | Descripción |
|-------|--------------|--------|-------------|
| EasyList | ~2 MB | ~49k | Bloqueo principal de anuncios |
| EasyPrivacy | ~1.4 MB | ~49k | Protección contra trackers |
| Fanboy Annoyances | ~1.7 MB | — | Elementos molestos (newsletters, banners) |
| Fanboy Anti Newsletter | ~1.7 MB | — | Popups de suscripción |
| Fanboy Social | ~405 KB | — | Widgets de redes sociales |
| Fanboy Mobile Notifications | ~1.7 MB | — | Notificaciones push intrusivas |

## Testing

### Verificar funcionamiento básico

```bash
# Dominio normal → debe resolver normalmente
dig @127.0.0.1 google.com +short

# Dominio publicitario → debe retornar 0.0.0.0
dig @127.0.0.1 ad.doubleclick.net +short
dig @127.0.0.1 googleads.g.doubleclick.net +short
```

### Logs en tiempo real

```bash
journalctl -u dns-adblocker -f
# Buscar líneas con BLOCKED: para ver qué se está bloqueando
```

## Estructura del proyecto

```
dns-adblocker/
├── src/
│   ├── main.rs           # Entry point
│   ├── config.rs         # Carga y estructura de configuración
│   ├── filter_lists.rs   # Manager de filtros y worker del engine
│   └── dns_server.rs     # Servidor UDP async + resolución upstream
├── Cargo.toml            # Dependencias
├── .gitignore            # Exclusión de backups y binarios
└── README.md
```

### Código por módulo

| Módulo | Líneas | Responsabilidad |
|--------|--------|-----------------|
| `main.rs` | 50 | Inicialización y orquestación |
| `config.rs` | 74 | Parseo de JSON + defaults |
| `filter_lists.rs` | 181 | Descarga/parseo de filtros + worker Tokio |
| `dns_server.rs` | 228 | Socket UDP async + query handling |
| **Total** | **533** | — |

## Seguridad

- Solo escucha en localhost por defecto (`127.0.0.1`)
- No almacena credenciales ni tokens
- Las listas de filtros se descargan vía HTTPS
- Memoria peak observada: ~45 MB

## Problemas resueltos

| Problema | Causa raíz | Solución |
|----------|-----------|----------|
| TIMEOUTs masivos | `recv()` bloqueante paralizaba el reactor Tokio | Reemplazar por `tokio::net::UdpSocket` + async/await completo |
| Bloqueos de queries | `mpsc::Receiver::recv()` también bloqueante | Migrar a `tokio::sync::oneshot` |
| AA flag incorrecto | Se enviaba AA=1 en respuestas de bloqueo | Remover AA; solo QR=1 + RA=1 |
| Buffer太小 | 4096 bytes no suficiente para DNS response grande | Aumentar a 65535 bytes |

## Depuración

```bash
# Ver logs detallados
RUST_LOG=dns_adblocker=debug journalctl -u dns-adblocker -f

# Probar un dominio específico manualmente
dig @127.0.0.1 -p 53 www.speedtest.net +time=3 +tries=1

# Ver memoria del proceso
ps aux | grep dns-adblocker | awk '{printf "%.0fMB\n", $6/1024}'
```

## Author

luxder
