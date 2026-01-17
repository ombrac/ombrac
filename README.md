# Ombrac

[![docs][docs-badge]][docs-url]
[![crates][crates-badge]][crates-url]
[![Build Status][ci-badge]][ci-url]
[![Build Status][release-badge]][release-url]
[![Apache 2.0 Licensed][license-badge]][license-url]

**Ombrac** is a high-performance, secure TCP-over-QUIC tunnel implemented in Rust

## Features
- **Secure**: Native TLS encryption built into the QUIC layer
- **High Performance**: Low-latency multiplexing via QUIC bidirectional streams
- **Versatile**: Supports SOCKS5, HTTP/HTTPS, and TUN device modes
- **Zero-RTT**: Supports 0-RTT and 0.5-RTT for near-instant connections

## Installation
The easiest way to get started is to download the latest pre-compiled binary from the [Releases Page](https://github.com/ombrac/ombrac/releases).

### Get Started
- **Homebrew**: `brew tap ombrac/tap && brew install ombrac`  
- **Docker**: `docker pull ghcr.io/ombrac/ombrac/ombrac-server:latest`
- **Cargo**: `cargo install ombrac-client ombrac-server --features binary`  

### Build from Source
```bash
git clone https://github.com/ombrac/ombrac.git && cd ombrac
cargo build --release --bin ombrac-client --bin ombrac-server --features binary
```

## Usage
### Server Setup
```bash
ombrac-server -l "[::]:443" -k "secret-key" --tls-cert "fullchain.pem" --tls-key "key.pem"
```

### Client Setup
```bash
ombrac-client -s "server:443" -k "secret-key" --socks "127.0.0.1:1080" --http "127.0.0.1:8080"
```

### Docker Deployment (Insecure Mode)
```bash
# Server
docker run -d --name ombrac-server -p 443:443/udp ghcr.io/ombrac/ombrac/ombrac-server:latest \
  -l 0.0.0.0:443 -k secret --tls-mode insecure

# Client
docker run -d --name ombrac-client -p 1080:1080/tcp ghcr.io/ombrac/ombrac/ombrac-client:latest \
  -s server:443 -k secret --socks 0.0.0.0:1080 --tls-mode insecure
```

## CLI
```bash
Usage: ombrac-client [OPTIONS]

Options:
  -c, --config <FILE>  Path to the JSON configuration file
  -h, --help           Print help
  -V, --version        Print version

Required:
  -k, --secret <STR>   Protocol Secret
  -s, --server <ADDR>  Address of the server to connect to

Protocol:
      --auth-option <STR>  Extended parameter of the protocol, used for authentication related information

Endpoint:
      --http <ADDR>             The address to bind for the HTTP/HTTPS server
      --socks <ADDR>            The address to bind for the SOCKS server
      --tun-fd <FD>             Use a pre-existing TUN device by providing its file descriptor `tun_ipv4`, `tun_ipv6`, and `tun_mtu` will be ignored
      --tun-ipv4 <CIDR>         The IPv4 address and subnet for the TUN device, in CIDR notation
      --tun-ipv6 <CIDR>         The IPv6 address and subnet for the TUN device, in CIDR notation
      --tun-mtu <MTU>           The Maximum Transmission Unit (MTU) for the TUN device. [default: 1500]
      --fake-dns <CIDR>         The IPv4 address pool for the built-in fake DNS server, in CIDR notation. [default: 198.18.0.0/16]
      --disable-udp-443 <BOOL>  Disable UDP traffic to port 443 [possible values: true, false]

Transport:
      --bind <ADDR>                 The address to bind for transport
      --server-name <STR>           Name of the server to connect (derived from `server` if not provided)
      --tls-mode <TLS_MODE>         Set the TLS mode for the connection [possible values: tls, m-tls, insecure]
      --ca-cert <FILE>              Path to the Certificate Authority (CA) certificate file in 'TLS' mode, if not provided, the system's default root certificates are used
      --client-cert <FILE>          Path to the client's TLS certificate for mTLS
      --client-key <FILE>           Path to the client's TLS private key for mTLS
      --zero-rtt <BOOL>             Enable 0-RTT for faster connection establishment [possible values: true, false]
      --alpn-protocols <PROTOCOLS>  Application-Layer protocol negotiation (ALPN) protocols [default: h3]
      --congestion <ALGORITHM>      Congestion control algorithm to use (e.g. bbr, cubic, newreno) [default: bbr]
      --cwnd-init <NUM>             Initial congestion window size in bytes
      --idle-timeout <TIME>         Maximum idle time (in milliseconds) before closing the connection [default: 30000]
      --keep-alive <TIME>           Keep-alive interval (in milliseconds) [default: 8000]
      --max-streams <NUM>           Maximum number of bidirectional streams that can be open simultaneously [default: 100]

Logging:
      --log-level <LEVEL>  Logging level (e.g., INFO, WARN, ERROR) [default: INFO]
```

```bash
Usage: ombrac-server [OPTIONS]

Options:
  -c, --config <FILE>  Path to the JSON configuration file
  -h, --help           Print help
  -V, --version        Print version

Required:
  -k, --secret <STR>   Protocol Secret
  -l, --listen <ADDR>  Address to bind the server

Transport:
      --tls-mode <TLS_MODE>         Set the TLS mode for the connection [possible values: tls, m-tls, insecure]
      --ca-cert <FILE>              Path to the Certificate Authority (CA) certificate file in 'TLS' mode, if not provided, the system's default root certificates are used
      --tls-cert <FILE>             Path to the server's TLS certificate
      --tls-key <FILE>              Path to the server's TLS private key
      --zero-rtt <BOOL>             Enable 0-RTT for faster connection establishment [possible values: true, false]
      --alpn-protocols <PROTOCOLS>  Application-Layer protocol negotiation (ALPN) protocols [default: h3]
      --congestion <ALGORITHM>      Congestion control algorithm to use (e.g. bbr, cubic, newreno) [default: bbr]
      --cwnd-init <NUM>             Initial congestion window size in bytes
      --idle-timeout <TIME>         Maximum idle time (in milliseconds) before closing the connection [default: 30000]
      --keep-alive <TIME>           Keep-alive interval (in milliseconds) [default: 8000]
      --max-streams <NUM>           Maximum number of bidirectional streams that can be open simultaneously [default: 1000]

Logging:
      --log-level <LEVEL>  Logging level (e.g., INFO, WARN, ERROR) [default: INFO]
```

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
