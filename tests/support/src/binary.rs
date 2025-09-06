use std::process::Command;

use crate::{path::BinaryLocator, process::ProcessGuard};

#[derive(Debug, Default, Clone)]
pub struct Client {
    pub secret: Option<String>,
    pub socks: Option<String>,
    pub server: Option<String>,
    pub tls_mode: Option<String>,
    pub ca_cert: Option<String>,
    pub client_cert: Option<String>,
    pub client_key: Option<String>,
}

impl Client {
    pub fn secret(mut self, secret: String) -> Self {
        self.secret = Some(secret);
        self
    }
    pub fn socks(mut self, socks: String) -> Self {
        self.socks = Some(socks);
        self
    }
    pub fn server(mut self, addr: String) -> Self {
        self.server = Some(addr);
        self
    }
    pub fn tls_mode(mut self, mode: String) -> Self {
        self.tls_mode = Some(mode);
        self
    }
    pub fn ca_cert(mut self, ca_cert: String) -> Self {
        self.ca_cert = Some(ca_cert);
        self
    }
    pub fn client_cert(mut self, cert: String) -> Self {
        self.client_cert = Some(cert);
        self
    }
    pub fn client_key(mut self, key: String) -> Self {
        self.client_key = Some(key);
        self
    }

    pub fn start(self) -> ProcessGuard {
        let opts = self;
        let mut args = Vec::new();

        if let Some(secret) = opts.secret {
            args.extend_from_slice(&["--secret".to_string(), secret]);
        }
        if let Some(socks) = opts.socks {
            args.extend_from_slice(&["--socks".to_string(), socks]);
        }
        if let Some(addr) = opts.server {
            args.extend_from_slice(&["--server".to_string(), addr]);
        }
        if let Some(mode) = opts.tls_mode {
            args.extend_from_slice(&["--tls-mode".to_string(), mode]);
        }
        if let Some(ca_cert) = opts.ca_cert {
            args.extend_from_slice(&["--ca-cert".to_string(), ca_cert]);
        }
        if let Some(cert) = opts.client_cert {
            args.extend_from_slice(&["--client-cert".to_string(), cert]);
        }
        if let Some(key) = opts.client_key {
            args.extend_from_slice(&["--client-key".to_string(), key]);
        }

        let client = Command::new(BinaryLocator::locate("ombrac-client"))
            .args(&args)
            .arg("--log-level")
            .arg("DEBUG")
            .spawn()
            .expect("Failed to start ombrac-client");
        ProcessGuard(client)
    }
}

#[derive(Debug, Default)]
pub struct Server {
    pub secret: Option<String>,
    pub listen: Option<String>,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    pub tls_mode: Option<String>,
    pub ca_cert: Option<String>,
}

impl Server {
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
    pub fn tls_mode(mut self, mode: String) -> Self {
        self.tls_mode = Some(mode);
        self
    }
    pub fn tls_ca(mut self, ca_cert: String) -> Self {
        self.ca_cert = Some(ca_cert);
        self
    }

    pub fn start(self) -> ProcessGuard {
        let opts = self;
        let mut args = Vec::new();

        if let Some(secret) = opts.secret {
            args.extend_from_slice(&["--secret".to_string(), secret]);
        }
        if let Some(listen) = opts.listen {
            args.extend_from_slice(&["--listen".to_string(), listen]);
        }
        if let Some(mode) = opts.tls_mode {
            args.extend_from_slice(&["--tls-mode".to_string(), mode]);
        }
        if let Some(tls_cert) = opts.tls_cert {
            args.extend_from_slice(&["--tls-cert".to_string(), tls_cert]);
        }
        if let Some(tls_key) = opts.tls_key {
            args.extend_from_slice(&["--tls-key".to_string(), tls_key]);
        }
        if let Some(ca_cert) = opts.ca_cert {
            args.extend_from_slice(&["--ca-cert".to_string(), ca_cert]);
        }

        let server = Command::new(BinaryLocator::locate("ombrac-server"))
            .args(&args)
            .arg("--log-level")
            .arg("DEBUG")
            .spawn()
            .expect("Failed to start ombrac-server");
        ProcessGuard(server)
    }
}
