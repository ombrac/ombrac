# Ombrac

**Ombrac** is a high-performance, Rust-based TCP tunneling system, designed to provide secure and efficient communication channels between clients and servers. By leveraging the asynchronous power of Tokio and providing modular support for protocols like SOCKS5, it can serve as a foundation for proxying and tunneling applications.

[![Apache 2.0 Licensed][license-badge]][license-url]
[![Build Status][actions-badge]][actions-url]

## Example
### Server
```shell
ombrac-server -l [::]:443 --tls-cert ./cert.pem --tls-key ./key.pem
```
It will starts the ombrac server listening on port 443, using the provided TLS certificate and key for encrypted communication.

### Client
```shell
ombrac-client --socks 127.0.0.1:1080 --server-address example.com:443
```
It will starts a SOCKS5 server on 127.0.0.1:1080, forwarding traffic to example.com:443.

## Contributing
Contributions are welcome! Feel free to fork the repository, submit issues, or send pull requests to help improve Ombrac.

## License
This project is licensed under the [Apache-2.0 License](./LICENSE).

[license-badge]: https://img.shields.io/badge/license-apache-blue.svg
[license-url]: https://github.com/ombrac/ombrac/blob/main/LICENSE
[actions-badge]: https://github.com/ombrac/ombrac/workflows/CI/badge.svg
[actions-url]: https://github.com/ombrac/ombrac/actions/workflows/ci.yml?query=branch%3Amain