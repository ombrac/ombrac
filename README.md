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

### Quick Install
- **Homebrew**: `brew tap ombrac/tap && brew install ombrac`  
- **Docker**: `docker pull ghcr.io/ombrac/ombrac/ombrac-server:latest`
- **Cargo**: `cargo install ombrac-client ombrac-server --features binary`  

### From Source
```bash
git clone https://github.com/ombrac/ombrac.git && cd ombrac
cargo build --release --bin ombrac-client --bin ombrac-server --features binary
```

## Quick Start
### Start Server
```bash
ombrac-server -l "[::]:443" -k "secret-key" --tls-cert "fullchain.pem" --tls-key "key.pem"
```

### Start Client (SOCKS5 Endpoint)
```bash
ombrac-client -s "server:443" -k "secret-key" --socks "127.0.0.1:1080"
```

### Run with Docker (Testing Mode)
```bash
# Server
docker run -d --name ombrac-server -p 443:443/udp ghcr.io/ombrac/ombrac/ombrac-server:latest \
  -l 0.0.0.0:443 -k secret --tls-mode insecure

# Client
docker run -d --name ombrac-client -p 1080:1080/tcp ghcr.io/ombrac/ombrac/ombrac-client:latest \
  -s server:443 -k secret --socks 0.0.0.0:1080 --tls-mode insecure
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
