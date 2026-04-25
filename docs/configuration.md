# Configuration Reference

Both binaries accept a JSON configuration file via `-c / --config`. CLI flags override the corresponding JSON fields.

## TLS Certificates

Ombrac supports three TLS modes, controlled by `tls_mode`:

| Mode | Description |
|------|-------------|
| `tls` | Standard TLS. The server requires `tls_cert` + `tls_key`. The client verifies the server certificate against system roots, or a custom CA via `ca_cert`. |
| `m-tls` | Mutual TLS. Both sides present certificates. The server additionally requires `ca_cert` to verify clients; the client requires `client_cert` + `client_key`. |
| `insecure` | Skips certificate verification entirely. For testing only — never use in production. |

**Obtaining a certificate**

Any publicly trusted certificate works (e.g. Let's Encrypt via `certbot` or `acme.sh`). Pass the resulting files as `tls_cert` (full chain PEM) and `tls_key` (private key PEM) on the server side.

For self-signed or private CA setups, generate a CA and sign a server certificate with it, then distribute the CA certificate to clients via `ca_cert`. Tools like [`rcgen`](https://github.com/rustls/rcgen) or `openssl` can automate this.

---

## Server

```json
{
  "secret": "your-secret",
  "listen": "0.0.0.0:443",
  "transport": {
    "tls_mode": "tls",
    "tls_cert": "fullchain.pem",
    "tls_key": "privkey.pem",
    "zero_rtt": false,
    "congestion": "bbr",
    "idle_timeout": 30000,
    "keep_alive": 8000,
    "max_streams": 1000
  },
  "connection": {
    "max_connections": 1024,
    "auth_timeout_secs": 15
  },
  "logging": {
    "log_level": "INFO"
  }
}
```

### Server Fields

| Field | Type | Description | Default |
|-------|------|-------------|---------|
| `secret` | string | Shared secret for authentication | *(required)* |
| `listen` | string | Address the server binds to | *(required)* |

**`transport`**

| Field | Type | Description | Default |
|-------|------|-------------|---------|
| `tls_mode` | string | `tls`, `m-tls`, or `insecure` | `tls` |
| `tls_cert` | string | Server TLS certificate path (PEM) | |
| `tls_key` | string | Server TLS private key path (PEM) | |
| `ca_cert` | string | CA certificate for mTLS client verification | |
| `zero_rtt` | bool | Enable 0-RTT fast reconnect | `false` |
| `alpn_protocols` | string | ALPN protocol list | `h3` |
| `congestion` | string | `bbr`, `cubic`, or `newreno` | `bbr` |
| `cwnd_init` | integer | Initial congestion window (bytes) | |
| `idle_timeout` | integer | Idle timeout before closing connection (ms) | `30000` |
| `keep_alive` | integer | Keep-alive interval (ms) | `8000` |
| `max_streams` | integer | Max simultaneous bidirectional streams | `1000` |

**`connection`**

| Field | Type | Description | Default |
|-------|------|-------------|---------|
| `max_connections` | integer | Maximum number of concurrent connections | `1024` |
| `auth_timeout_secs` | integer | Seconds to wait for client authentication | `15` |

**`logging`**

| Field | Type | Description | Default |
|-------|------|-------------|---------|
| `log_level` | string | `ERROR`, `WARN`, `INFO`, `DEBUG`, or `TRACE` | `INFO` |

---

## Client

```json
{
  "secret": "your-secret",
  "server": "your-server:443",
  "auth_option": "",
  "endpoint": {
    "socks": "127.0.0.1:1080",
    "http": "127.0.0.1:8080",
    "tun": {
      "tun_ipv4": "10.0.0.1/8",
      "tun_ipv6": "fd00::1/8",
      "tun_mtu": 1500,
      "fake_dns": "198.18.0.0/16",
      "disable_udp_443": false
    }
  },
  "transport": {
    "tls_mode": "tls",
    "ca_cert": "ca.pem",
    "zero_rtt": false,
    "congestion": "bbr",
    "idle_timeout": 30000,
    "keep_alive": 8000,
    "max_streams": 100
  },
  "logging": {
    "log_level": "INFO"
  }
}
```

### Client Fields

| Field | Type | Description | Default |
|-------|------|-------------|---------|
| `secret` | string | Shared secret for authentication | *(required)* |
| `server` | string | Server address to connect to | *(required)* |
| `auth_option` | string | Extended authentication parameter | |

**`endpoint`**

| Field | Type | Description | Default |
|-------|------|-------------|---------|
| `socks` | string | Bind address for SOCKS5 proxy | |
| `http` | string | Bind address for HTTP/HTTPS proxy | |
| `tun.tun_ipv4` | string | IPv4 address/subnet for the TUN device (CIDR) | |
| `tun.tun_ipv6` | string | IPv6 address/subnet for the TUN device (CIDR) | |
| `tun.tun_mtu` | integer | MTU for the TUN device | `1500` |
| `tun.fake_dns` | string | IPv4 pool for the built-in fake DNS server (CIDR) | `198.18.0.0/16` |
| `tun.disable_udp_443` | bool | Disable UDP traffic to port 443 | `false` |

**`transport`**

| Field | Type | Description | Default |
|-------|------|-------------|---------|
| `tls_mode` | string | `tls`, `m-tls`, or `insecure` | `tls` |
| `ca_cert` | string | CA certificate path; uses system roots if omitted | |
| `client_cert` | string | Client certificate path for mTLS | |
| `client_key` | string | Client private key path for mTLS | |
| `server_name` | string | TLS server name override | *(derived from `server`)* |
| `bind` | string | Local address to bind the QUIC transport | |
| `zero_rtt` | bool | Enable 0-RTT fast reconnect | `false` |
| `alpn_protocols` | string | ALPN protocol list | `h3` |
| `congestion` | string | `bbr`, `cubic`, or `newreno` | `bbr` |
| `cwnd_init` | integer | Initial congestion window (bytes) | |
| `idle_timeout` | integer | Idle timeout before closing connection (ms) | `30000` |
| `keep_alive` | integer | Keep-alive interval (ms) | `8000` |
| `max_streams` | integer | Max simultaneous bidirectional streams | `100` |

**`logging`**

| Field | Type | Description | Default |
|-------|------|-------------|---------|
| `log_level` | string | `ERROR`, `WARN`, `INFO`, `DEBUG`, or `TRACE` | `INFO` |
