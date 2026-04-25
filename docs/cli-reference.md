# CLI Reference

## `ombrac-server`

```
Usage: ombrac-server [OPTIONS]

Options:
  -c, --config <FILE>   Path to the JSON configuration file
  -h, --help            Print help
  -V, --version         Print version
```

### General

| Flag | Description | Default |
|------|-------------|---------|
| `-k, --secret <STR>` | Shared secret for authentication | *(required)* |
| `-l, --listen <ADDR>` | Address the server binds to | *(required)* |

### Transport

| Flag | Description | Default |
|------|-------------|---------|
| `--tls-mode <MODE>` | TLS mode: `tls`, `m-tls`, `insecure` | `tls` |
| `--tls-cert <FILE>` | Server TLS certificate (PEM) | |
| `--tls-key <FILE>` | Server TLS private key (PEM) | |
| `--ca-cert <FILE>` | CA certificate for mTLS client verification | |
| `--zero-rtt <BOOL>` | Enable 0-RTT fast reconnect | `false` |
| `--alpn-protocols <STR>` | ALPN protocol list | `h3` |
| `--congestion <ALG>` | Congestion algorithm: `bbr`, `cubic`, `newreno` | `bbr` |
| `--cwnd-init <BYTES>` | Initial congestion window size | |
| `--idle-timeout <MS>` | Idle timeout before closing connection | `30000` |
| `--keep-alive <MS>` | Keep-alive interval | `8000` |
| `--max-streams <NUM>` | Max simultaneous bidirectional streams | `1000` |

### Logging

| Flag | Description | Default |
|------|-------------|---------|
| `--log-level <LEVEL>` | Log level: `ERROR`, `WARN`, `INFO`, `DEBUG`, `TRACE` | `INFO` |

---

## `ombrac-client`

```
Usage: ombrac-client [OPTIONS]

Options:
  -c, --config <FILE>   Path to the JSON configuration file
  -h, --help            Print help
  -V, --version         Print version
```

### General

| Flag | Description | Default |
|------|-------------|---------|
| `-k, --secret <STR>` | Shared secret for authentication | *(required)* |
| `-s, --server <ADDR>` | Server address to connect to | *(required)* |
| `--auth-option <STR>` | Extended authentication parameter | |

### Endpoint

| Flag | Description | Default |
|------|-------------|---------|
| `--socks <ADDR>` | Bind address for SOCKS5 proxy | |
| `--http <ADDR>` | Bind address for HTTP/HTTPS proxy | |
| `--tun-fd <FD>` | Use a pre-existing TUN device by file descriptor | |
| `--tun-ipv4 <CIDR>` | IPv4 address/subnet for the TUN device | |
| `--tun-ipv6 <CIDR>` | IPv6 address/subnet for the TUN device | |
| `--tun-mtu <MTU>` | MTU for the TUN device | `1500` |
| `--fake-dns <CIDR>` | IPv4 pool for the built-in fake DNS server | `198.18.0.0/16` |
| `--disable-udp-443 <BOOL>` | Disable UDP traffic to port 443 | `false` |

### Transport

| Flag | Description | Default |
|------|-------------|---------|
| `--bind <ADDR>` | Local address to bind the QUIC transport | |
| `--server-name <STR>` | TLS server name (derived from `--server` if omitted) | |
| `--tls-mode <MODE>` | TLS mode: `tls`, `m-tls`, `insecure` | `tls` |
| `--ca-cert <FILE>` | CA certificate; uses system roots if omitted | |
| `--client-cert <FILE>` | Client certificate for mTLS | |
| `--client-key <FILE>` | Client private key for mTLS | |
| `--zero-rtt <BOOL>` | Enable 0-RTT fast reconnect | `false` |
| `--alpn-protocols <STR>` | ALPN protocol list | `h3` |
| `--congestion <ALG>` | Congestion algorithm: `bbr`, `cubic`, `newreno` | `bbr` |
| `--cwnd-init <BYTES>` | Initial congestion window size | |
| `--idle-timeout <MS>` | Idle timeout before closing connection | `30000` |
| `--keep-alive <MS>` | Keep-alive interval | `8000` |
| `--max-streams <NUM>` | Max simultaneous bidirectional streams | `100` |

### Logging

| Flag | Description | Default |
|------|-------------|---------|
| `--log-level <LEVEL>` | Log level: `ERROR`, `WARN`, `INFO`, `DEBUG`, `TRACE` | `INFO` |
