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

Alternatively, you can use the `--insecure` option to skip TLS verification. **This is not recommended for production environments as it bypasses certificate validation, potentially exposing your communication to security risks.**


## Full Options

### Server

```shell
Usage: ombrac-server [OPTIONS] --secret <STR> --listen <ADDR>

Options:
  -h, --help     Print help
  -V, --version  Print version

Service Secret:
  -k, --secret <STR>  Protocol Secret

Transport QUIC:
  -l, --listen <ADDR>        The address to bind for QUIC transport
      --tls-cert <FILE>      Path to the TLS certificate file
      --tls-key <FILE>       Path to the TLS private key file
      --insecure             When enabled, the server will generate a self-signed TLS certificate
                             and use it for the QUIC connection. This mode is useful for testing
                             but should not be used in production
      --zero-rtt             Enable 0-RTT for faster connection establishment (may reduce security)
      --cwnd-init <NUM>      Initial congestion window size in bytes
      --idle-timeout <TIME>  Maximum idle time (in milliseconds) before closing the connection
                             30 second default recommended by RFC 9308 [default: 30000]
      --keep-alive <TIME>    Keep-alive interval (in milliseconds) [default: 8000]
      --max-streams <NUM>    Maximum number of bidirectional streams that can be open simultaneously [default: 100]

Logging:
      --log-level <LEVEL>  Logging level (e.g., INFO, WARN, ERROR) [default: WARN]
      --log-dir <PATH>     Path to the log directory
      --log-prefix <STR>   Prefix for log file names (only used when log-dir is specified) [default: log]
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
      --socks <ADDR>  The address to bind for the SOCKS server [default: 127.0.0.1:1080]

Transport QUIC:
      --bind <ADDR>          The address to bind for QUIC transport
  -s, --server <ADDR>        Address of the server to connect to
      --server-name <STR>    Name of the server to connect (derived from `server` if not provided)
      --tls-cert <FILE>      Path to the TLS certificate file
      --insecure             Skip TLS certificate verification (insecure, for testing only)
      --zero-rtt             Enable 0-RTT for faster connection establishment (may reduce security)
      --no-multiplex         Disable connection multiplexing (each stream uses a separate QUIC connection)
                             This may be useful in special network environments where multiplexing causes issues
      --cwnd-init <NUM>      Initial congestion window size in bytes
      --idle-timeout <TIME>  Maximum idle time (in milliseconds) before closing the connection
                             30 second default recommended by RFC 9308 [default: 30000]
      --keep-alive <TIME>    Keep-alive interval (in milliseconds) [default: 8000]
      --max-streams <NUM>    Maximum number of bidirectional streams that can be open simultaneously [default: 100]

Logging:
      --log-level <LEVEL>  Logging level (e.g., INFO, WARN, ERROR) [default: WARN]
      --log-dir <PATH>     Path to the log directory
      --log-prefix <STR>   Prefix for log file names (only used when log-dir is specified) [default: log]
```

## License
This project is licensed under the [Apache-2.0 License](./LICENSE).

[license-badge]: https://img.shields.io/badge/license-apache-blue.svg
[license-url]: https://github.com/ombrac/ombrac/blob/main/LICENSE
[ci-badge]: https://github.com/ombrac/ombrac/workflows/CI/badge.svg
[ci-url]: https://github.com/ombrac/ombrac/actions/workflows/ci.yml?query=branch%3Amain
[release-badge]: https://github.com/ombrac/ombrac/workflows/Release/badge.svg
[release-url]: https://github.com/ombrac/ombrac/actions/workflows/release.yml?query=branch%3Amain
