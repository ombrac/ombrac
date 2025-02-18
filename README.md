# Ombrac

**Ombrac** is a high-performance, Rust-based TCP tunneling solution designed for secure communication

## Features
- **High Performance**: Leverages QUIC's multiplexing capabilities with bidirectional streams for efficient and low-latency transmission.
- **Secure Communication**: Encryption is ensured by the built-in TLS layer of QUIC, providing robust security for your data.
- **Zero-RTT Support**: Optional 0-RTT or 0.5-RTT connections for faster handshakes (at the cost of slightly weakened security).

[![Apache 2.0 Licensed][license-badge]][license-url]
[![Build Status][ci-badge]][ci-url]
[![Build Status][release-badge]][release-url]



## Install
### Releases
Download the latest release from the [releases page](https://github.com/ombrac/ombrac/releases).



### Build
```shell
cargo build --bin ombrac-client --bin ombrac-server --features binary
```

**NOTE**: On linux systems, [`aws-lc-rs`](https://github.com/aws/aws-lc-rs) will be used for cryptographic operations. A C compiler and CMake may be required on these systems for installation.

### Crates
```shell
cargo install ombrac-client ombrac-server --features binary
```

### Homebrew
```shell
brew tap ombrac/tap && brew install ombrac
```


## Usage
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

When using a self-signed certificate, the client requires both the `--server-name` parameter and the `--tls-cert` path to be explicitly configured. 

Alternatively, you can use the `--tls-skip` option to skip TLS verification. **This is not recommended for production environments as it bypasses certificate validation, potentially exposing your communication to security risks.**


## Usage

### Server

```shell
Usage: ombrac-server [OPTIONS] --secret <STR> --listen <ADDR>

Options:
  -h, --help     Print help
  -V, --version  Print version

Service Secret:
  -k, --secret <STR>  Protocol Secret

Transport QUIC:
  -l, --listen <ADDR>
          Transport server listening address
      --tls-cert <FILE>
          Path to the TLS certificate file for secure connections
      --tls-key <FILE>
          Path to the TLS private key file for secure connections
      --tls-skip
          When enabled, a self-signed certificate and key will be generated, the cert and key will be disregarded
      --enable-zero-rtt
          Whether to enable 0-RTT or 0.5-RTT connections at the cost of weakened security
      --congestion-initial-window <NUM>
          Initial congestion window in bytes
      --max-idle-timeout <TIME>
          Connection idle timeout in millisecond
      --max-keep-alive-period <TIME>
          Connection keep alive period in millisecond
      --max-open-bidirectional-streams <NUM>
          Connection max open bidirectional streams

Logging:
      --tracing-level <TRACE>  Logging level e.g., INFO, WARN, ERROR [default: WARN]
```

### Client
```shell
Usage: ombrac-client [OPTIONS] --secret <STR> --server <ADDR>

Options:
  -h, --help     Print help
  -V, --version  Print version

Service Secret:
  -k, --secret <STR>  Protocol Secret

Endpoint SOCKS:
      --socks <ADDR>  Listening address for the SOCKS server [default: 127.0.0.1:1080]

Transport QUIC:
      --bind <ADDR>
          Bind address
  -s, --server <ADDR>
          Address of the server to connect
      --server-name <STR>
          Name of the server to connect
      --tls-cert <FILE>
          Path to the TLS certificate file for secure connections
      --tls-skip
          Skip TLS verification for connections
      --enable-zero-rtt
          Whether to enable 0-RTT or 0.5-RTT connections at the cost of weakened security
      --enable-connection-multiplexing
          Whether to enable connection multiplexing
      --congestion-initial-window <NUM>
          Initial congestion window in bytes
      --max-idle-timeout <TIME>
          Connection idle timeout in millisecond
      --max-keep-alive-period <TIME>
          Connection keep alive period in millisecond [default: 8000]
      --max-open-bidirectional-streams <NUM>
          Connection max open bidirectional streams

Logging:
      --tracing-level <TRACE>  Logging level e.g., INFO, WARN, ERROR [default: WARN]
```

## License
This project is licensed under the [Apache-2.0 License](./LICENSE).

[license-badge]: https://img.shields.io/badge/license-apache-blue.svg
[license-url]: https://github.com/ombrac/ombrac/blob/main/LICENSE
[ci-badge]: https://github.com/ombrac/ombrac/workflows/CI/badge.svg
[ci-url]: https://github.com/ombrac/ombrac/actions/workflows/ci.yml?query=branch%3Amain
[release-badge]: https://github.com/ombrac/ombrac/workflows/Release/badge.svg
[release-url]: https://github.com/ombrac/ombrac/actions/workflows/release.yml?query=branch%3Amain
