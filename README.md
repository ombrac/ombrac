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

## CLI
### Server
```bash
Usage: ombrac-server [OPTIONS] --secret <STR> --listen <ADDR>

Options:
  -h, --help     Print help
  -V, --version  Print version

Required:
  -k, --secret <STR>   Protocol Secret
  -l, --listen <ADDR>  The address to bind for transport

Transport:
      --tls-mode <TLS_MODE>         Set the TLS mode for the connection
                                    tls: Standard TLS. The client verifies the server's certificate
                                    m-tls: Mutual TLS with client and server certificate verification
                                    insecure: Generates a self-signed certificate on the fly with `SANs` set to `localhost` (for testing only) [default: tls] [possible values: tls, m-tls, insecure]
      --ca-cert <FILE>              Path to the Certificate Authority (CA) certificate file
                                    Used in 'mTLS' modes
      --tls-cert <FILE>             Path to the TLS certificate file
      --tls-key <FILE>              Path to the TLS private key file
      --zero-rtt                    Enable 0-RTT for faster connection establishment (may reduce security)
      --alpn-protocols <PROTOCOLS>  Application-Layer protocol negotiation (ALPN) protocols
                                    e.g. "h3,h3-29" [default: h3]
      --congestion <ALGORITHM>      Congestion control algorithm to use (e.g. bbr, cubic, newreno) [default: bbr]
      --cwnd-init <NUM>             Initial congestion window size in bytes
      --idle-timeout <TIME>         Maximum idle time (in milliseconds) before closing the connection
                                    30 second default recommended by RFC 9308 [default: 30000]
      --keep-alive <TIME>           Keep-alive interval (in milliseconds) [default: 8000]
      --max-streams <NUM>           Maximum number of bidirectional streams that can be open simultaneously [default: 1000]

Logging:
      --log-level <LEVEL>  Logging level (e.g., INFO, WARN, ERROR) [default: WARN]
      --log-dir <PATH>     Path to the log directory
      --log-prefix <STR>   Prefix for log file names (only used when log-dir is specified) [default: log]
```

### Client
```bash
Usage: ombrac-client [OPTIONS] --secret <STR> --server <ADDR>

Options:
  -h, --help     Print help
  -V, --version  Print version

Required:
  -k, --secret <STR>   Protocol Secret
  -s, --server <ADDR>  Address of the server to connect to

Endpoint:
      --http <ADDR>   The address to bind for the HTTP/HTTPS server
      --socks <ADDR>  The address to bind for the SOCKS server [default: 127.0.0.1:1080]

Transport:
      --bind <ADDR>                 The address to bind for transport
      --server-name <STR>           Name of the server to connect (derived from `server` if not provided)
      --tls-mode <TLS_MODE>         Set the TLS mode for the connection
                                    tls: Standard TLS with server certificate verification
                                    m-tls: Mutual TLS with client and server certificate verification
                                    insecure: Skip server certificate verification (for testing only) [default: tls] [possible values: tls, m-tls, insecure]
      --ca-cert <FILE>              Path to the Certificate Authority (CA) certificate file
                                    in 'TLS' mode, if not provided, the system's default root certificates are used
      --client-cert <FILE>          Path to the client's TLS certificate for mTLS
      --client-key <FILE>           Path to the client's TLS private key for mTLS
      --zero-rtt                    Enable 0-RTT for faster connection establishment (may reduce security)
      --alpn-protocols <PROTOCOLS>  Application-Layer protocol negotiation (ALPN) protocols
                                    e.g. "h3,h3-29" [default: h3]
      --congestion <ALGORITHM>      Congestion control algorithm to use (e.g. bbr, cubic, newreno) [default: bbr]
      --cwnd-init <NUM>             Initial congestion window size in bytes
      --idle-timeout <TIME>         Maximum idle time (in milliseconds) before closing the connection
                                    30 second default recommended by RFC 9308 [default: 30000]
      --keep-alive <TIME>           Keep-alive interval (in milliseconds) [default: 8000]
      --max-streams <NUM>           Maximum number of bidirectional streams that can be open simultaneously [default: 100]
  -4, --prefer-ipv4                 Try to resolve domain name to IPv4 addresses first
  -6, --prefer-ipv6                 Try to resolve domain name to IPv6 addresses first

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
