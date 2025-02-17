use std::process::Command;

use crate::{path::BinaryLocator, process::ProcessGuard};

#[derive(Debug, Default, Clone)]
pub struct ClientBuilder {
    pub secret: Option<String>,
    pub socks: Option<String>,
    pub tls_cert: Option<String>,
    pub tls_skip: bool,
    pub server_name: Option<String>,
    pub server: Option<String>,
}

impl ClientBuilder {
    pub fn secret(mut self, secret: String) -> Self {
        self.secret = Some(secret);
        self
    }

    pub fn socks(mut self, socks: String) -> Self {
        self.socks = Some(socks);
        self
    }

    pub fn tls_cert(mut self, cert: String) -> Self {
        self.tls_cert = Some(cert);
        self
    }

    pub fn tls_skip(mut self, skip: bool) -> Self {
        self.tls_skip = skip;
        self
    }

    pub fn server_name(mut self, name: String) -> Self {
        self.server_name = Some(name);
        self
    }

    pub fn server(mut self, addr: String) -> Self {
        self.server = Some(addr);
        self
    }

    pub fn build(self) -> ProcessGuard {
        Client::start(Some(self))
    }
}

pub struct Client;

impl Client {
    pub fn start(options: Option<ClientBuilder>) -> ProcessGuard {
        let args = options
            .map(|opts| {
                let mut args = Vec::new();
                if let Some(secret) = opts.secret {
                    args.extend_from_slice(&["--secret".to_string(), secret]);
                }
                if let Some(socks) = opts.socks {
                    args.extend_from_slice(&["--socks".to_string(), socks]);
                }
                if let Some(cert) = opts.tls_cert {
                    args.extend_from_slice(&["--tls-cert".to_string(), cert]);
                }
                if opts.tls_skip {
                    args.extend_from_slice(&["--tls-skip".to_string()]);
                }
                if let Some(name) = opts.server_name {
                    args.extend_from_slice(&["--server-name".to_string(), name]);
                }
                if let Some(addr) = opts.server {
                    args.extend_from_slice(&["--server".to_string(), addr]);
                }
                args
            })
            .unwrap_or_default();

        let client = Command::new(BinaryLocator::locate("ombrac-client"))
            .args(&args)
            .arg("--tracing-level")
            .arg("DEBUG")
            .spawn()
            .expect("Failed to start ombrac-client");

        ProcessGuard(client)
    }
}

#[derive(Debug, Default)]
pub struct ServerBuilder {
    pub secret: Option<String>,
    pub listen: Option<String>,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    pub tls_skip: bool,
}

impl ServerBuilder {
    pub fn secret(mut self, secret: String) -> Self {
        self.secret = Some(secret);
        self
    }

    pub fn listen(mut self, listen: String) -> Self {
        self.listen = Some(listen);
        self
    }

    pub fn tls_cert(mut self, cert: String) -> Self {
        self.tls_cert = Some(cert);
        self
    }

    pub fn tls_key(mut self, key: String) -> Self {
        self.tls_key = Some(key);
        self
    }

    pub fn tls_skip(mut self, skip: bool) -> Self {
        self.tls_skip = skip;
        self
    }

    pub fn build(self) -> ProcessGuard {
        Server::start(Some(self))
    }
}

pub struct Server;

impl Server {
    pub fn start(options: Option<ServerBuilder>) -> ProcessGuard {
        let args = options
            .map(|opts| {
                let mut args = Vec::new();
                if let Some(secret) = opts.secret {
                    args.extend_from_slice(&["--secret".to_string(), secret]);
                }
                if let Some(listen) = opts.listen {
                    args.extend_from_slice(&["--listen".to_string(), listen]);
                }
                if let Some(tls_cert) = opts.tls_cert {
                    args.extend_from_slice(&["--tls-cert".to_string(), tls_cert]);
                }
                if let Some(tls_key) = opts.tls_key {
                    args.extend_from_slice(&["--tls-key".to_string(), tls_key]);
                }
                if opts.tls_skip {
                    args.extend_from_slice(&["--tls-skip".to_string()]);
                }
                args
            })
            .unwrap_or_default();

        let server = Command::new(BinaryLocator::locate("ombrac-server"))
            .args(&args)
            .arg("--tracing-level")
            .arg("DEBUG")
            .spawn()
            .expect("Failed to start ombrac-server");

        ProcessGuard(server)
    }
}
