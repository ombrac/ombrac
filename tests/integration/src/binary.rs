use std::process::Command;

use tests_support::{path::BinaryLocator, process::ProcessGuard};

#[derive(Debug, Default, Clone)]
pub struct ClientBuilder {
    pub socks: Option<String>,
    pub tls_cert: Option<String>,
    pub server_name: Option<String>,
    pub server_address: Option<String>,
}

impl ClientBuilder {
    pub fn socks(mut self, socks: String) -> Self {
        self.socks = Some(socks);
        self
    }

    pub fn tls_cert(mut self, cert: String) -> Self {
        self.tls_cert = Some(cert);
        self
    }

    pub fn server_name(mut self, name: String) -> Self {
        self.server_name = Some(name);
        self
    }

    pub fn server_address(mut self, address: String) -> Self {
        self.server_address = Some(address);
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
                if let Some(socks) = opts.socks {
                    args.extend_from_slice(&["--socks".to_string(), socks]);
                }
                if let Some(cert) = opts.tls_cert {
                    args.extend_from_slice(&["--tls-cert".to_string(), cert]);
                }
                if let Some(name) = opts.server_name {
                    args.extend_from_slice(&["--server-name".to_string(), name]);
                }
                if let Some(addr) = opts.server_address {
                    args.extend_from_slice(&["--server-address".to_string(), addr]);
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
    pub listen: Option<String>,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
}

impl ServerBuilder {
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
                if let Some(listen) = opts.listen {
                    args.extend_from_slice(&["--listen".to_string(), listen]);
                }
                if let Some(tls_cert) = opts.tls_cert {
                    args.extend_from_slice(&["--tls-cert".to_string(), tls_cert]);
                }
                if let Some(tls_key) = opts.tls_key {
                    args.extend_from_slice(&["--tls-key".to_string(), tls_key]);
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
