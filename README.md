# Ombrac

**Ombrac** is a high-performance, Rust-based TCP tunneling solution designed for secure communication

## Features
- **High Performance**: Leverages QUIC's multiplexing capabilities with bidirectional streams for efficient and low-latency transmission.
- **Secure Communication**: Encryption is ensured by the built-in TLS layer of QUIC.
- **Zero-RTT Support**: Optional 0-RTT or 0.5-RTT connections for faster handshakes (at the cost of slightly weakened security).

[![Apache 2.0 Licensed][license-badge]][license-url]
[![Build Status][ci-badge]][ci-url]
[![Build Status][release-badge]][release-url]

## Structure 
The codebase is organized into focused crates

| Crate                  | Description                                                                 |
|------------------------|-----------------------------------------------------------------------------|
| `crates/ombrac`        | Core protocol implementation                                                |
| `crates/ombrac-client` | Client binary with HTTP/SOCKS proxy entry points                            |
| `crates/ombrac-server` | Server binary (wrapper around the core protocol)                            |
| `crates/ombrac-transport` | QUIC transport layer implementation                                      |


## Install
### Releases
Download the latest release from the [releases page](https://github.com/ombrac/ombrac/releases).

### Build
```shell
cargo build --bin ombrac-client --bin ombrac-server
```

**NOTE**: On linux systems, [`aws-lc-rs`](https://github.com/aws/aws-lc-rs) will be used for cryptographic operations. A C compiler and CMake may be required on these systems for installation.

### Crates
```shell
cargo install ombrac-client ombrac-server
```

### Homebrew
```shell
brew tap ombrac/tap && brew install ombrac
```

### Docker
#### Pull from GitHub Container Registry
```shell
docker pull ghcr.io/ombrac/ombrac/ombrac-server:latest
docker pull ghcr.io/ombrac/ombrac/ombrac-client:latest
```

## QuickStart
### Server
```shell
ombrac-server -l "[::]:443" -k "secret" --tls-cert "./cert.pem" --tls-key "./key.pem"
```
Starts the Ombrac server listening on port 443, using the provided TLS certificate and key for encrypted communication.

### Client
```shell
ombrac-client -s "example.com:443" -k "secret"
```
Will sets up a SOCKS5 server on 127.0.0.1:1080, forwarding traffic to example.com:443.

Alternatively, you can use the `--tls-mode insecure` option to skip TLS verification. **This is not recommended for production environments as it bypasses certificate validation, potentially exposing your communication to security risks.**


### Run the container
```shell
docker run --name ombrac-server \
  --restart always \
  -p 2098:2098/udp \
  -dit ghcr.io/ombrac/ombrac/ombrac-server:latest \
  -l 0.0.0.0:2098 \
  -k secret \
  --tls-mode insecure
```

```shell
docker run --name ombrac-client \
  --restart always \
  -p 1080:1080/tcp \
  -dit ghcr.io/ombrac/ombrac/ombrac-client:latest \
  -s example.com:2098 \
  -k secret \
  --socks 0.0.0.0:1080 \
  --log-level INFO \
  --tls-mode insecure
```

## License
This project is licensed under the [Apache-2.0 License](./LICENSE).

[license-badge]: https://img.shields.io/badge/license-apache-blue.svg
[license-url]: https://github.com/ombrac/ombrac/blob/main/LICENSE
[ci-badge]: https://github.com/ombrac/ombrac/workflows/CI/badge.svg
[ci-url]: https://github.com/ombrac/ombrac/actions/workflows/ci.yml?query=branch%3Amain
[release-badge]: https://github.com/ombrac/ombrac/workflows/Release/badge.svg
[release-url]: https://github.com/ombrac/ombrac/actions/workflows/release.yml?query=branch%3Amain
