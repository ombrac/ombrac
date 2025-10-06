# Ombrac

**Ombrac** is a high-performance, Rust-based TCP tunneling solution designed for secure communication

## Features
- **High Performance**: Leverages QUIC's multiplexing capabilities with bidirectional streams for efficient and low-latency transmission.
- **Secure Communication**: Encryption is ensured by the built-in TLS layer of QUIC.
- **Zero-RTT Support**: Optional 0-RTT or 0.5-RTT connections for faster handshakes (at the cost of slightly weakened security).

[![Apache 2.0 Licensed][license-badge]][license-url]
[![Build Status][ci-badge]][ci-url]
[![Build Status][release-badge]][release-url]

## Architecture
```
+----------+      +-------------------+      +===============+      +---------------+      +-----------------+
| Your App |----->|   Ombrac Client   |----->|   Encrypted   |----->| Ombrac Server |----->| Target Internet |
|          |<-----| (SOCKS5/HTTP/TUN) |<-----| （QUIC/Other） |<-----|               |<-----|                 |
+----------+      +-------------------+      +===============+      +---------------+      +-----------------+
```

## Installation
The easiest way to get started is to download the latest pre-compiled binary from the [Releases Page](https://github.com/ombrac/ombrac/releases).


### Homebrew (macOS & Linux)
```shell
brew tap ombrac/tap && brew install ombrac
```

### From Crates.io
```shell
cargo install ombrac-client ombrac-server --features binary
```

### From Source
```shell
# Clone the repository
git clone https://github.com/ombrac/ombrac.git && cd ombrac

# Build the binaries
cargo build --release --bin ombrac-client --bin ombrac-server --features binary
```

> **NOTE**: On linux systems, [`aws-lc-rs`](https://github.com/aws/aws-lc-rs) will be used for cryptographic operations. A C compiler and CMake may be required on these systems for installation.


### Docker
Pull from GitHub Container Registry
```shell
# Pull the server image
docker pull ghcr.io/ombrac/ombrac/ombrac-server:latest

# Pull the client image
docker pull ghcr.io/ombrac/ombrac/ombrac-client:latest
```

## Getting Started
### Run the Server
```shell
ombrac-server \
  -l "[::]:443" \
  -k "your-secret-key" \
  --tls-cert "/path/to/your/cert.pem" \
  --tls-key "/path/to/your/key.pem" \
  --log-level INFO
```

- `-l`: The address to listen on.
- `-k`: The secret key for the protocol.
- `--tls-cert` & `--tls-key`: Paths to your TLS certificate and private key.

### Run the Client
```shell
ombrac-client \
  -s "your-server:443" \
  -k "your-secret-key" \
  --socks "127.0.0.1:1080" \
  --log-level INFO
```

- `-s`: The server address to connect to.  
- `-k`: The same secret key used on the server.  
- `--socks`: The local address to bind the SOCKS5 proxy to.  

> ⚠️ Security Warning   
> For testing, you can use `--tls-mode insecure` on the client to skip certificate validation. This is highly discouraged for production environments as it exposes your connection to man-in-the-middle attacks.

### Example with Docker
**Server Container**
```shell
docker run --name ombrac-server \
  --restart always \
  -p 443:443/udp \
  -dit ghcr.io/ombrac/ombrac/ombrac-server:latest \
  -l 0.0.0.0:443 \
  -k secret \
  --tls-mode insecure
```

**Client Container**
```shell
docker run --name ombrac-client \
  --restart always \
  -p 1080:1080/tcp \
  -dit ghcr.io/ombrac/ombrac/ombrac-client:latest \
  -s example.com:443 \
  -k secret \
  --socks 0.0.0.0:1080 \
  --log-level INFO \
  --tls-mode insecure
```

## CLI
### Server
```
Usage: ombrac-server [OPTIONS]

Options:
  -c, --config <FILE>  Path to the JSON configuration file
  -h, --help           Print help
  -V, --version        Print version

Required:
  -k, --secret <STR>   Protocol Secret
  -l, --listen <ADDR>  The address to bind for transport

Transport:
      --tls-mode <TLS_MODE>         Set the TLS mode for the connection tls: Standard TLS with server certificate verification m-tls: Mutual TLS with client and server certificate verification insecure: Generates a self-signed certificate for testing (SANs set to 'localhost') [possible values: tls, m-tls, insecure]
      --ca-cert <FILE>              Path to the Certificate Authority (CA) certificate file for mTLS
      --tls-cert <FILE>             Path to the TLS certificate file
      --tls-key <FILE>              Path to the TLS private key file
      --zero-rtt <ZERO_RTT>         Enable 0-RTT for faster connection establishment (may reduce security) [possible values: true, false]
      --alpn-protocols <PROTOCOLS>  Application-Layer protocol negotiation (ALPN) protocols [default: h3]
      --congestion <ALGORITHM>      Congestion control algorithm to use (e.g. bbr, cubic, newreno) [default: bbr]
      --cwnd-init <NUM>             Initial congestion window size in bytes
      --idle-timeout <TIME>         Maximum idle time (in milliseconds) before closing the connection [default: 30000]
      --keep-alive <TIME>           Keep-alive interval (in milliseconds) [default: 8000]
      --max-streams <NUM>           Maximum number of bidirectional streams that can be open simultaneously [default: 1000]

Logging:
      --log-level <LEVEL>  Logging level (e.g., INFO, WARN, ERROR) [default: INFO]
      --log-dir <PATH>     Path to the log directory
      --log-prefix <STR>   Prefix for log file names (only used when log dir is specified)

```

### Client
```
Usage: ombrac-client [OPTIONS]

Options:
  -c, --config <FILE>  Path to the JSON configuration file
  -h, --help           Print help
  -V, --version        Print version

Required:
  -k, --secret <STR>   Protocol Secret
  -s, --server <ADDR>  Address of the server to connect to

Endpoint:
      --http <ADDR>      The address to bind for the HTTP/HTTPS server
      --socks <ADDR>     The address to bind for the SOCKS server
      --tun-fd <FD>      Use a pre-existing TUN device by providing its file descriptor. `tun_ipv4`, `tun_ipv6`, and `tun_mtu` will be ignored
      --tun-ipv4 <CIDR>  The IPv4 address and subnet for the TUN device, in CIDR notation (e.g., 198.19.0.1/24)
      --tun-ipv6 <CIDR>  The IPv6 address and subnet for the TUN device, in CIDR notation (e.g., fd00::1/64)
      --tun-mtu <U16>    The Maximum Transmission Unit (MTU) for the TUN device. [default: 1500]
      --fake-dns <CIDR>  The IPv4 address pool for the built-in fake DNS server, in CIDR notation. [default: 198.18.0.0/16]

Transport:
      --bind <ADDR>                 The address to bind for transport
      --server-name <STR>           Name of the server to connect (derived from `server` if not provided)
      --tls-mode <TLS_MODE>         Set the TLS mode for the connection tls: Standard TLS with server certificate verification m-tls: Mutual TLS with client and server certificate verification insecure: Skip server certificate verification (for testing only) [possible values: tls, m-tls, insecure]
      --ca-cert <FILE>              Path to the Certificate Authority (CA) certificate file in 'TLS' mode, if not provided, the system's default root certificates are used
      --client-cert <FILE>          Path to the client's TLS certificate for mTLS
      --client-key <FILE>           Path to the client's TLS private key for mTLS
      --zero-rtt <ZERO_RTT>         Enable 0-RTT for faster connection establishment (may reduce security) [possible values: true, false]
      --alpn-protocols <PROTOCOLS>  Application-Layer protocol negotiation (ALPN) protocols [default: h3]
      --congestion <ALGORITHM>      Congestion control algorithm to use (e.g. bbr, cubic, newreno) [default: bbr]
      --cwnd-init <NUM>             Initial congestion window size in bytes
      --idle-timeout <TIME>         Maximum idle time (in milliseconds) before closing the connection [default: 30000] 30 second default recommended by RFC 9308
      --keep-alive <TIME>           Keep-alive interval (in milliseconds) [default: 8000]
      --max-streams <NUM>           Maximum number of bidirectional streams that can be open simultaneously [default: 100]

Logging:
      --log-level <LEVEL>  Logging level (e.g., INFO, WARN, ERROR) [default: INFO]
      --log-dir <PATH>     Path to the log directory
      --log-prefix <STR>   Prefix for log file names (only used when log dir is specified)

```

## License
This project is licensed under the [Apache-2.0 License](./LICENSE).

[license-badge]: https://img.shields.io/badge/license-apache-blue.svg
[license-url]: https://github.com/ombrac/ombrac/blob/main/LICENSE
[ci-badge]: https://github.com/ombrac/ombrac/workflows/CI/badge.svg
[ci-url]: https://github.com/ombrac/ombrac/actions/workflows/ci.yml?query=branch%3Amain
[release-badge]: https://github.com/ombrac/ombrac/workflows/Release/badge.svg
[release-url]: https://github.com/ombrac/ombrac/actions/workflows/release.yml?query=branch%3Amain
