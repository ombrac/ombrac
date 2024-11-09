# Ombrac

**Ombrac** is a high-performance, Rust-based TCP tunneling solution designed for secure communication between clients and servers. It is ideal for developers and network administrators seeking efficient and secure data transmission.

## Features
- Optionally pass through SOCKS
- Encryption is ensured by the built-in TLS layer of QUIC
- Employs QUIC multiplexing with bidirectional streams for efficient transmission

[![Apache 2.0 Licensed][license-badge]][license-url]
[![Build Status][actions-badge]][actions-url]
[![Telegram Group][telegram-group-badge]][telegram-group-url]


## Install
### Binaries
Download the latest release from the [releases page](https://github.com/ombrac/ombrac/releases).

## Example
### Server
```shell
ombrac-server -l [::]:443 --tls-cert ./cert.pem --tls-key ./key.pem
```
This command starts the Ombrac server listening on port 443, using the provided TLS certificate and key for encrypted communication.

### Client
```shell
ombrac-client --socks 127.0.0.1:1080 --server-address example.com:443
```
This command sets up a SOCKS5 server on 127.0.0.1:1080, forwarding traffic to example.com:443.

## Usage
### Server

```shell
Usage: ombrac-server [OPTIONS] --listen <ADDR> --tls-cert <FILE> --tls-key <FILE>

Options:
  -h, --help     Print help
  -V, --version  Print version

Transport QUIC:
  -l, --listen <ADDR>
          Transport server listening address
      --tls-cert <FILE>
          Path to the TLS certificate file for secure connections
      --tls-key <FILE>
          Path to the TLS private key file for secure connections
      --initial-congestion-window <NUM>
          Initial congestion window in bytes [default: 32]
      --max-handshake-duration <TIME>
          Handshake timeout in millisecond [default: 3000]
      --max-idle-timeout <TIME>
          Connection idle timeout in millisecond [default: 0]
      --max-keep-alive-period <TIME>
          Connection keep alive period in millisecond [default: 8000]
      --max-open-bidirectional-streams <NUM>
          Connection max open bidirectional streams [default: 100]
      --bidirectional-local-data-window <NUM>
          Bidirectional stream local data window [default: 3750000]
      --bidirectional-remote-data-window <NUM>
          Bidirectional stream remote data window [default: 3750000]

DNS:
      --dns <ENUM>  Domain name system resolver [default: cloudflare] [possible values: cloudflare, cloudflare-tls, google, google-tls]

Logging:
      --tracing-level <TRACE>  Logging level e.g., INFO, WARN, ERROR [default: WARN]
```

### Client
```shell
Usage: ombrac-client [OPTIONS] --server-address <ADDR>

Options:
  -h, --help     Print help
  -V, --version  Print version

Endpoint SOCKS:
      --socks <ADDR>  Listening address for the SOCKS server [default: 127.0.0.1:1080]

Transport QUIC:
      --bind <ADDR>
          Bind local address
      --server-name <STR>
          Name of the server to connect to
      --server-address <ADDR>
          Address of the server to connect to
      --tls-cert <FILE>
          Path to the TLS certificate file for secure connections
      --initial-congestion-window <NUM>
          Initial congestion window in bytes [default: 32]
      --max-multiplex <NUM>
          Connection multiplexing [default: 0]
      --max-multiplex-interval <TIME>
          Connection multiplexing interval in millisecond [default: 60000]
      --max-multiplex-per-interval <NUM>
          Connection multiplexing allowed within a specific interval [default: 16]
      --max-handshake-duration <TIME>
          Handshake timeout in millisecond [default: 3000]
      --max-idle-timeout <TIME>
          Connection idle timeout in millisecond [default: 0]
      --max-keep-alive-period <TIME>
          Connection keep alive period in millisecond [default: 8000]
      --max-open-bidirectional-streams <NUM>
          Connection max open bidirectional streams [default: 100]
      --bidirectional-local-data-window <NUM>
          Bidirectional stream local data window [default: 3750000]
      --bidirectional-remote-data-window <NUM>
          Bidirectional stream remote data window [default: 3750000]

Logging:
      --tracing-level <TRACE>  Logging level e.g., INFO, WARN, ERROR [default: WARN]
```

## Contributing
Contributions are welcome! Feel free to fork the repository, submit issues, or send pull requests to help improve Ombrac.

## License
This project is licensed under the [Apache-2.0 License](./LICENSE).

[license-badge]: https://img.shields.io/badge/license-apache-blue.svg
[license-url]: https://github.com/ombrac/ombrac/blob/main/LICENSE
[actions-badge]: https://github.com/ombrac/ombrac/workflows/CI/badge.svg
[actions-url]: https://github.com/ombrac/ombrac/actions/workflows/ci.yml?query=branch%3Amain
[telegram-group-badge]: https://img.shields.io/badge/Telegram-2CA5E0?style=flat-squeare&logo=telegram&logoColor=white
[telegram-group-url]: https://t.me/ombrac_group