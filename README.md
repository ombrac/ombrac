# Ombrac

[![docs][docs-badge]][docs-url]
[![crates][crates-badge]][crates-url]
[![Build Status][ci-badge]][ci-url]
[![Build Status][release-badge]][release-url]
[![Apache 2.0 Licensed][license-badge]][license-url]

**Ombrac** is a high-performance, secure TCP/UDP-over-QUIC tunnel implemented in Rust. It multiplexes traffic over a single authenticated QUIC connection, supports SOCKS5, HTTP/HTTPS proxy, and TUN device modes on the client, and is designed for both server deployments and mobile embedding via FFI.

## Features

- **Secure by default** — TLS 1.3 encryption via QUIC; supports standard TLS, mutual TLS (mTLS), and an insecure mode for testing
- **High performance** — BBR congestion control, QUIC stream multiplexing, 0-RTT/0.5-RTT fast reconnect
- **Versatile client endpoints** — SOCKS5, HTTP/HTTPS proxy, TUN device with built-in fake-DNS
- **UDP tunneling** — Full UDP proxy support with session management and fragment reassembly
- **Graceful operation** — Automatic reconnect with back-off, SIGTERM-aware shutdown, configurable idle timeouts
- **Embeddable** — C-compatible FFI interface for iOS/Android integration

## Installation

Download the latest pre-compiled binary from the [Releases Page](https://github.com/ombrac/ombrac/releases), or use one of the methods below.

```bash
# Homebrew
brew tap ombrac/tap && brew install ombrac

# Cargo
cargo install ombrac-client ombrac-server --features binary

# Docker
docker pull ghcr.io/ombrac/ombrac/ombrac-server:latest
docker pull ghcr.io/ombrac/ombrac/ombrac-client:latest
```

### Build from Source

```bash
git clone https://github.com/ombrac/ombrac.git && cd ombrac
cargo build --release --bin ombrac-client --bin ombrac-server --features binary
```

## Quick Start

### Server

```bash
ombrac-server \
  --listen "[::]:443" \
  --secret "your-secret" \
  --tls-cert fullchain.pem \
  --tls-key privkey.pem
```

### Client

```bash
# SOCKS5 proxy on :1080 and HTTP proxy on :8080
ombrac-client \
  --server "your-server:443" \
  --secret "your-secret" \
  --socks "127.0.0.1:1080" \
  --http "127.0.0.1:8080"
```

### Docker (Insecure Mode — for testing only)

```bash
# Server
docker run -d \
  --name ombrac-server \
  -p 443:443/udp \
  ghcr.io/ombrac/ombrac/ombrac-server:latest \
  --listen 0.0.0.0:443 \
  --secret secret \
  --tls-mode insecure

# Client
docker run -d \
  --name ombrac-client \
  -p 1080:1080/tcp \
  ghcr.io/ombrac/ombrac/ombrac-client:latest \
  --server remote:443 \
  --secret secret \
  --socks 0.0.0.0:1080 \
  --tls-mode insecure
```

## Configuration

Both binaries accept a JSON configuration file via `-c / --config`. CLI flags override the corresponding JSON fields.

### Server Configuration

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

### Client Configuration

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

## CLI Reference

### `ombrac-server`

```
Usage: ombrac-server [OPTIONS]

Options:
  -c, --config <FILE>   Path to the JSON configuration file
  -h, --help            Print help
  -V, --version         Print version
```

| Flag | Description | Default |
|------|-------------|---------|
| `-k, --secret <STR>` | Shared secret for authentication | *(required)* |
| `-l, --listen <ADDR>` | Address the server binds to | *(required)* |

**Transport**

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

**Logging**

| Flag | Description | Default |
|------|-------------|---------|
| `--log-level <LEVEL>` | Log level: `ERROR`, `WARN`, `INFO`, `DEBUG`, `TRACE` | `INFO` |

---

### `ombrac-client`

```
Usage: ombrac-client [OPTIONS]

Options:
  -c, --config <FILE>   Path to the JSON configuration file
  -h, --help            Print help
  -V, --version         Print version
```

| Flag | Description | Default |
|------|-------------|---------|
| `-k, --secret <STR>` | Shared secret for authentication | *(required)* |
| `-s, --server <ADDR>` | Server address to connect to | *(required)* |
| `--auth-option <STR>` | Extended authentication parameter | |

**Endpoint**

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

**Transport**

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

**Logging**

| Flag | Description | Default |
|------|-------------|---------|
| `--log-level <LEVEL>` | Log level: `ERROR`, `WARN`, `INFO`, `DEBUG`, `TRACE` | `INFO` |

## License

This project is licensed under the [Apache-2.0 License](./LICENSE).

[docs-badge]: https://docs.rs/ombrac/badge.svg
[docs-url]: https://docs.rs/ombrac
[crates-badge]: https://img.shields.io/badge/crates.io-ombrac-orange
[crates-url]: https://crates.io/crates/ombrac
[license-badge]: https://img.shields.io/badge/license-apache-blue.svg
[license-url]: https://github.com/ombrac/ombrac/blob/main/LICENSE
[ci-badge]: https://github.com/ombrac/ombrac/workflows/CI/badge.svg
[ci-url]: https://github.com/ombrac/ombrac/actions/workflows/ci.yml?query=branch%3Amain
[release-badge]: https://github.com/ombrac/ombrac/workflows/Release/badge.svg
[release-url]: https://github.com/ombrac/ombrac/actions/workflows/release.yml?query=branch%3Amain
