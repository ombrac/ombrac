# Ombrac

[![docs][docs-badge]][docs-url]
[![crates][crates-badge]][crates-url]
[![Build Status][ci-badge]][ci-url]
[![Build Status][release-badge]][release-url]
[![Apache 2.0 Licensed][license-badge]][license-url]

A secure, high-performance TCP/UDP-over-QUIC tunnel written in Rust.

## Highlights

- TLS 1.3 encryption with optional mutual TLS — secure by default
- BBR congestion control, stream multiplexing, 0-RTT fast reconnect
- SOCKS5, HTTP/HTTPS proxy, and TUN device endpoints
- Full UDP tunneling with fragment reassembly
- Automatic reconnect, configurable idle timeouts, SIGTERM-aware shutdown
- C FFI interface for iOS/Android embedding

## Installation

```bash
# Homebrew
brew tap ombrac/tap && brew install ombrac

# Cargo
cargo install ombrac-client ombrac-server --features binary
```

Pre-compiled binaries are available on the [Releases page](https://github.com/ombrac/ombrac/releases).

## Quick Start

**Server**

```bash
ombrac-server \
  --listen "[::]:443" \
  --secret "your-secret" \
  --tls-cert fullchain.pem \
  --tls-key privkey.pem
```

**Client**

```bash
ombrac-client \
  --server "your-server:443" \
  --secret "your-secret" \
  --socks "127.0.0.1:1080"
```

## Documentation

- [Configuration Reference](./docs/configuration.md)
- [CLI Reference](./docs/cli-reference.md)

## License

[Apache-2.0](./LICENSE)

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
