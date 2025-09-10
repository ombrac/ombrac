use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::Arc;

use ombrac_macros::{debug, error};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use tokio::sync::watch;

#[cfg(feature = "datagram")]
use crate::Unreliable;
use crate::quic::TransportConfig;
#[cfg(feature = "datagram")]
use crate::quic::datagram::{Datagram, Session};
use crate::quic::stream::Stream;
use crate::{Acceptor, Reliable};

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

impl Default for Config {
    fn default() -> Self {
        Self::new()
    }
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

    pub fn transport_config(&mut self, config: TransportConfig) {
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

pub struct Server {
    endpoint: Arc<quinn::Endpoint>,
    stream_receiver: async_channel::Receiver<Stream>,
    #[cfg(feature = "datagram")]
    datagram_receiver: async_channel::Receiver<Datagram>,
    shutdown_sender: watch::Sender<()>,
}

impl Server {
    pub async fn new(config: Config, socket: UdpSocket) -> Result<Self> {
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

        let (stream_sender, stream_receiver) = async_channel::unbounded();
        #[cfg(feature = "datagram")]
        let (datagram_sender, datagram_receiver) = async_channel::unbounded();
        let (shutdown_sender, shutdown_receiver) = watch::channel(());

        tokio::spawn(run(
            endpoint.clone(),
            stream_sender,
            #[cfg(feature = "datagram")]
            datagram_sender,
            shutdown_receiver,
        ));

        Ok(Self {
            endpoint,
            stream_receiver,
            #[cfg(feature = "datagram")]
            datagram_receiver,
            shutdown_sender,
        })
    }

    pub fn shutdown(&self) {
        let _ = self.shutdown_sender.send(());
    }

    pub async fn accept_bidirectional(&self) -> Result<Stream> {
        match self.stream_receiver.recv().await {
            Ok(value) => Ok(value),
            Err(_err) => Err(Error::ConnectionClosed),
        }
    }

    #[cfg(feature = "datagram")]
    pub async fn accept_datagram(&self) -> Result<Datagram> {
        match self.datagram_receiver.recv().await {
            Ok(value) => Ok(value),
            Err(_err) => Err(Error::ConnectionClosed),
        }
    }

    pub fn close(&self) {
        self.endpoint.close(0u32.into(), b"Server closed");
    }

    /// Get the local SocketAddr the underlying socket is bound to
    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.endpoint.local_addr()?)
    }

    /// Switch to a new UDP socket
    pub async fn rebind(&self, socket: UdpSocket) -> Result<()> {
        Ok(self.endpoint.rebind(socket)?)
    }

    /// Wait for all connections on the endpoint to be cleanly shut down
    pub async fn wait_idle(&self) -> () {
        self.endpoint.wait_idle().await
    }
}

impl Acceptor for Server {
    async fn accept_bidirectional(&self) -> io::Result<impl Reliable> {
        Ok(Server::accept_bidirectional(self).await?)
    }

    #[cfg(feature = "datagram")]
    async fn accept_datagram(&self) -> io::Result<impl Unreliable> {
        Ok(Server::accept_datagram(self).await?)
    }
}

async fn run(
    endpoint: Arc<quinn::Endpoint>,
    stream_sender: async_channel::Sender<Stream>,
    #[cfg(feature = "datagram")] datagram_sender: async_channel::Sender<Datagram>,
    mut shutdown_receiver: watch::Receiver<()>,
) {
    loop {
        tokio::select! {
            _ = shutdown_receiver.changed() => {
                break;
            }

            maybe_conn = endpoint.accept() => {
                match maybe_conn {
                    Some(conn) => {
                        tokio::spawn(handle_connection(
                            conn,
                            stream_sender.clone(),
                            #[cfg(feature = "datagram")]
                            datagram_sender.clone(),
                        ));
                    }
                    None => {
                        break;
                    }
                }
            }
        }
    }

    endpoint.wait_idle().await;
}

async fn handle_connection(
    conn: quinn::Incoming,
    stream_sender: async_channel::Sender<Stream>,
    #[cfg(feature = "datagram")] datagram_sender: async_channel::Sender<Datagram>,
) {
    match conn.await {
        Ok(connection) => {
            let _remote_addr = connection.remote_address();
            debug!("Connection from {}", _remote_addr);

            #[cfg(feature = "datagram")]
            {
                let session = Session::with_server(connection.clone());
                tokio::spawn(async move {
                    while let Some(datagram) = session.accept_datagram().await {
                        if datagram_sender.send(datagram).await.is_err() {
                            break;
                        }
                    }
                });
            }

            while let Ok((send_stream, recv_stream)) = connection.accept_bi().await {
                if stream_sender
                    .send(Stream(send_stream, recv_stream))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }
        Err(_e) => {
            error!("Failed to accept connection: {_e}")
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown();
    }
}
