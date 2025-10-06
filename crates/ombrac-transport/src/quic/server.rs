use std::io;
use std::net::SocketAddr;
use std::net::UdpSocket;
use std::path::PathBuf;
use std::sync::Arc;

use async_channel::{Receiver, Sender};
use ombrac_macros::{debug, error, warn};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use tokio::sync::watch;

use super::error::{Error, Result};

#[derive(Debug, Clone)]
pub struct Config {
    pub enable_zero_rtt: bool,
    pub enable_self_signed: bool,
    pub alpn_protocols: Vec<Vec<u8>>,
    pub root_ca_path: Option<PathBuf>,
    pub tls_cert_key_paths: Option<(PathBuf, PathBuf)>,

    transport_config: Arc<quinn::TransportConfig>,
}

impl Config {
    pub fn new() -> Self {
        Self {
            tls_cert_key_paths: None,
            root_ca_path: None,
            enable_zero_rtt: false,
            enable_self_signed: false,
            alpn_protocols: Vec::new(),
            transport_config: Arc::new(quinn::TransportConfig::default()),
        }
    }

    pub fn transport_config(&mut self, config: crate::quic::TransportConfig) {
        self.transport_config = Arc::new(config.0)
    }

    fn build_endpoint_config(&self) -> Result<quinn::EndpointConfig> {
        Ok(quinn::EndpointConfig::default())
    }

    fn build_server_config(&self) -> Result<quinn::ServerConfig> {
        use quinn::crypto::rustls::QuicServerConfig;

        let server_crypto = self.build_tls_config()?;
        let mut server_config =
            quinn::ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(server_crypto)?));

        server_config.transport_config(self.transport_config.clone());

        Ok(server_config)
    }

    fn build_tls_config(&self) -> Result<rustls::ServerConfig> {
        let (certs, key) = if self.enable_self_signed {
            let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()])?;
            let key = PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der()).into();
            let certs = vec![CertificateDer::from(cert.cert)];
            (certs, key)
        } else {
            let (cert, key) = self
                .tls_cert_key_paths
                .as_ref()
                .ok_or(Error::ServerMissingCertificate)?;
            let certs = super::load_certificates(cert)?;
            let key = super::load_private_key(key)?;
            (certs, key)
        };

        let config_builder = rustls::ServerConfig::builder();

        let mut tls_config = if let Some(ca_path) = &self.root_ca_path {
            // Enable mTLS, Client auth
            let mut ca_store = rustls::RootCertStore::empty();
            let ca_certs = super::load_certificates(ca_path)?;
            ca_store.add_parsable_certificates(ca_certs);

            let verifier = rustls::server::WebPkiClientVerifier::builder(ca_store.into())
                .build()
                .map_err(io::Error::other)?;

            config_builder
                .with_client_cert_verifier(verifier)
                .with_single_cert(certs, key)?
        } else {
            config_builder
                .with_no_client_auth()
                .with_single_cert(certs, key)?
        };

        tls_config.alpn_protocols = self.alpn_protocols.clone();

        if self.enable_zero_rtt {
            tls_config.send_half_rtt_data = true;
            tls_config.max_early_data_size = u32::MAX;
        }

        Ok(tls_config)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Server {
    endpoint: Arc<quinn::Endpoint>,
    receiver: Receiver<quinn::Connection>,
    shutdown_sender: watch::Sender<()>,
}

impl Server {
    pub async fn new(socket: UdpSocket, config: Config) -> Result<Self> {
        let server_config = config.build_server_config()?;
        let endpoint_config = config.build_endpoint_config()?;

        let runtime =
            quinn::default_runtime().ok_or_else(|| io::Error::other("No async runtime found"))?;
        let endpoint = Arc::new(quinn::Endpoint::new_with_abstract_socket(
            endpoint_config,
            Some(server_config),
            runtime.wrap_udp_socket(socket)?,
            runtime,
        )?);

        let (sender, receiver) = async_channel::unbounded();
        let (shutdown_sender, shutdown_receiver) = watch::channel(());

        tokio::spawn(accept_loop(endpoint.clone(), sender, shutdown_receiver));

        Ok(Self {
            endpoint,
            receiver,
            shutdown_sender,
        })
    }
}

async fn accept_loop(
    endpoint: Arc<quinn::Endpoint>,
    sender: Sender<quinn::Connection>,
    mut shutdown_receiver: watch::Receiver<()>,
) {
    loop {
        let endpoint = endpoint.clone();
        let sender_clone = sender.clone();

        tokio::select! {
            Some(accept) = endpoint.accept() => {
                tokio::spawn(async move {
                    match accept.await {
                        Ok(connection) => {
                            debug!("Accept connection from {}", connection.remote_address());
                            if sender_clone.send(connection).await.is_err() {
                                warn!("Connection receiver is closed");
                            }
                        }
                        Err(_err) => {
                            error!("Connection error {}", _err);
                        }
                    }
                });
            }
            _ = shutdown_receiver.changed() => {
                endpoint.close(0u32.into(), b"Server shutting down");
                break;
            }
        }
    }
}

impl crate::Acceptor for Server {
    type Connection = quinn::Connection;

    async fn accept(&self) -> io::Result<Self::Connection> {
        match self.receiver.recv().await {
            Ok(conn) => Ok(conn),
            Err(_) => Err(io::Error::other("Acceptor is closed")),
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.endpoint.local_addr()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.shutdown_sender.send(());
    }
}
