# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

### Build

```bash
# Build all workspace members
cargo build

# Build the release binaries (requires `binary` feature)
cargo build --release --bin ombrac-client --bin ombrac-server --features binary

# Build with all features
cargo build --all-features

# Build with no default features
cargo build --no-default-features
```

### Test

```bash
# Run all tests
cargo test

# Run tests for a specific crate
cargo test -p ombrac
cargo test -p ombrac-client
cargo test -p tests-integration

# Run a single test by name
cargo test -p tests-integration tcp_edge_cases

# Run release-mode tests
cargo test --release
```

### iOS XCFramework

```bash
./scripts/build_xcframework_ombrac_client.sh
```

## Architecture

Ombrac is a TCP-over-QUIC tunnel. The workspace is organized into focused crates with clear dependency layers:

```
ombrac-client / ombrac-server
        ↓
ombrac-transport  (QUIC via quinn/rustls)
        ↓
ombrac            (protocol messages, codec, UDP reassembly)

ombrac-netstack   (userspace TCP/UDP stack via smoltcp — used by TUN endpoint only)
ombrac-macros     (tracing no-op shims when `tracing` feature is off)
```

### `crates/ombrac`

Protocol layer: defines the wire format (`protocol.rs`) with `ClientHello`, `ClientConnect`, `ServerConnectResponse`, `UdpPacket`, and `Address`. Messages are serialized with `bincode` and framed with length-delimited codec (`codec.rs`). UDP fragmentation/reassembly lives in `reassembly.rs`.

### `crates/ombrac-transport`

Transport abstraction layer. Defines three traits in `lib.rs`:
- `Initiator` — client side: `connect()` returns a `Connection`
- `Acceptor` — server side: `accept()` returns a `Connection`
- `Connection` — bidirectional stream + optional datagram support

The only current implementation is QUIC (`quic/` module) backed by `quinn` + `rustls` + `aws-lc-rs`.

### `crates/ombrac-client`

Client library and binary. Key modules:
- `connection/` — `ClientConnection` handles authentication handshake (sends `ClientHello`, waits for `ServerAuthResponse`), automatic reconnection with exponential backoff (1s–60s), and per-stream `ClientConnect` negotiation
- `client.rs` — `Client<T, C>` wraps `ClientConnection` and optionally a `UdpDispatcher` for datagram mode
- `endpoint/` — SOCKS5, HTTP/HTTPS proxy, and TUN device endpoints that feed into `Client`
- `service.rs` — `OmbracClient` high-level API that wires config → QUIC transport → `Client` → endpoints
- `ffi.rs` / `logging/ffi.rs` — C FFI bindings (used for iOS/Android)

### `crates/ombrac-server`

Server library and binary. Key modules:
- `connection/` — `ConnectionAcceptor` reads `ClientHello`, authenticates (blake3 hash of secret), then dispatches per-stream `ClientConnect` to TCP/UDP handlers
- `service.rs` — `OmbracServer` high-level API

### `crates/ombrac-netstack`

Userspace network stack used only by the TUN endpoint. Wraps `smoltcp` to implement a virtual network interface (`device.rs`/`stack.rs`) that bridges TUN packets to Tokio async TCP/UDP sockets (`tcp.rs`, `udp.rs`).

### Features

Both `ombrac-client` and `ombrac-server` share the same feature structure:
- `tracing` (default) — enables `tracing` crate; without it, `ombrac-macros` provides no-op macros
- `datagram` — enables QUIC datagram-based UDP forwarding
- `endpoint-socks`, `endpoint-http`, `endpoint-tun` — individual endpoint types (client only)
- `binary` / `full` / `ffi` — composite features for binary builds, full library builds, and FFI

### Protocol flow

**Handshake (once per QUIC connection):**
1. Client opens stream → sends `ClientMessage::Hello` (version + blake3(secret) + options)
2. Server replies `ServerAuthResponse::Ok` or `Err`

**Per-request (stream mode):**
1. Client opens new bidirectional stream → sends `ClientMessage::Connect(address)`
2. Server replies `ServerConnectResponse::Ok` or `Err{kind, message}`
3. Stream becomes raw TCP proxy

**UDP (datagram mode):**
- Client sends `UdpPacket` datagrams; large packets are fragmented and reassembled on the server side using `reassembly.rs`

### Config

Both binaries support a JSON config file (`-c config.json`) or CLI flags. Config structs are in `config/mod.rs` (`ServiceConfig`, `TransportConfig`, `EndpointConfig`). TLS modes: `tls` (system roots or custom CA), `m-tls` (mutual TLS), `insecure`.

### Cross-compilation

CI builds for Linux targets require `musl-tools`, `cmake`, `clang`, and cross-compilers. On Windows, NASM is required for `aws-lc-rs`. The `Dockerfile` shows the expected build environment for Linux musl targets.
