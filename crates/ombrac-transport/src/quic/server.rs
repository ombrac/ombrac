use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use ombrac_macros::{debug, error};
use quinn::crypto::rustls::QuicServerConfig;
use quinn::{Endpoint, ServerConfig};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use tokio::sync::watch;

#[cfg(feature = "datagram")]
use crate::Unreliable;
#[cfg(feature = "datagram")]
use crate::quic::datagram::{Datagram, Session};
use crate::quic::stream::Stream;
use crate::{Acceptor, Reliable};

use super::TransportConfig;
use super::error::{Error, Result};

#[derive(Debug)]
pub struct Builder {
    listen: SocketAddr,
    enable_zero_rtt: bool,
    enable_self_signed: bool,
    tls_paths: Option<(PathBuf, PathBuf)>,
}

impl Builder {
    pub fn new(listen: SocketAddr) -> Self {
        Self {
            listen,
            tls_paths: None,
            enable_zero_rtt: false,
            enable_self_signed: false,
        }
    }

    pub fn with_tls(&mut self, paths: (PathBuf, PathBuf)) -> &mut Self {
        self.tls_paths = Some(paths);
        self
    }

    pub fn with_enable_zero_rtt(&mut self, value: bool) -> &mut Self {
        self.enable_zero_rtt = value;
        self
    }

    pub fn with_enable_self_signed(&mut self, value: bool) -> &mut Self {
        self.enable_self_signed = value;
        self
    }

    pub async fn build(self, transport_config: TransportConfig) -> Result<QuicServer> {
        let server_config = self.build_server_config(transport_config.0)?;
        let endpoint = Endpoint::server(server_config, self.listen)?;

        let (stream_sender, stream_receiver) = async_channel::unbounded();
        #[cfg(feature = "datagram")]
        let (datagram_sender, datagram_receiver) = async_channel::unbounded();
        let (shutdown_sender, shutdown_receiver) = watch::channel(());

        tokio::spawn(run(
            endpoint,
            stream_sender,
            #[cfg(feature = "datagram")]
            datagram_sender,
            shutdown_receiver,
        ));

        Ok(QuicServer {
            stream_receiver,
            #[cfg(feature = "datagram")]
            datagram_receiver,
            shutdown_sender,
        })
    }

    fn build_server_config(
        &self,
        transport_config: quinn::TransportConfig,
    ) -> Result<ServerConfig> {
        let server_crypto = self.build_tls_config()?;
        let mut server_config =
            ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(server_crypto)?));

        server_config.transport_config(Arc::new(transport_config));

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
                .tls_paths
                .as_ref()
                .ok_or(Error::ServerMissingCertificate)?;
            let certs = super::load_certificates(cert)?;
            let key = super::load_private_key(key)?;
            (certs, key)
        };

        let mut config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)?;

        config.alpn_protocols = vec![b"h3".to_vec()];

        // Zero RTT
        if self.enable_zero_rtt {
            config.send_half_rtt_data = true;
            config.max_early_data_size = u32::MAX;
        }

        Ok(config)
    }
}

pub struct QuicServer {
    stream_receiver: async_channel::Receiver<Stream>,
    #[cfg(feature = "datagram")]
    datagram_receiver: async_channel::Receiver<Datagram>,
    shutdown_sender: watch::Sender<()>,
}

impl QuicServer {
    pub fn shutdown(&self) {
        let _ = self.shutdown_sender.send(());
    }
}

impl Acceptor for QuicServer {
    #[inline]
    async fn accept_bidirectional(&self) -> io::Result<impl Reliable> {
        self.stream_receiver.recv().await.map_err(io::Error::other)
    }

    #[cfg(feature = "datagram")]
    #[inline]
    async fn accept_datagram(&self) -> io::Result<impl Unreliable> {
        self.datagram_receiver
            .recv()
            .await
            .map_err(io::Error::other)
    }
}

async fn run(
    endpoint: quinn::Endpoint,
    stream_sender: async_channel::Sender<Stream>,
    #[cfg(feature = "datagram")] datagram_sender: async_channel::Sender<Datagram>,
    mut shutdown_receiver: watch::Receiver<()>,
) {
    loop {
        tokio::select! {
            _ = shutdown_receiver.changed() => {
                endpoint.close(0u32.into(), b"");
                endpoint.wait_idle().await;
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
}

async fn handle_connection(
    conn: quinn::Incoming,
    stream_sender: async_channel::Sender<Stream>,
    #[cfg(feature = "datagram")] datagram_sender: async_channel::Sender<Datagram>,
) {
    match conn.await {
        Ok(connection) => {
            let _remote_addr = connection.remote_address();
            debug!("QUIC Connection from {}", _remote_addr);

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
