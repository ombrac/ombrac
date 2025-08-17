use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ombrac_macros::{debug, error};
use quinn::crypto::rustls::QuicServerConfig;
use quinn::{Endpoint, ServerConfig, TransportConfig, VarInt};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use tokio::sync::watch;

#[cfg(feature = "datagram")]
use crate::Unreliable;
#[cfg(feature = "datagram")]
use crate::quic::datagram::{Datagram, Session};
use crate::quic::stream::Stream;
use crate::{Acceptor, Reliable};

use super::Congestion;
use super::error::{Error, Result};

#[derive(Debug)]
pub struct Builder {
    listen: SocketAddr,
    enable_zero_rtt: bool,
    enable_self_signed: bool,
    tls_paths: Option<(PathBuf, PathBuf)>,
    transport_config: TransportConfig,
}

impl Builder {
    pub fn new(listen: SocketAddr) -> Self {
        Self {
            listen,
            tls_paths: None,
            enable_zero_rtt: false,
            enable_self_signed: false,
            transport_config: TransportConfig::default(),
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

    pub fn with_congestion(
        &mut self,
        congestion: Congestion,
        initial_window: Option<u64>,
    ) -> &mut Self {
        use quinn::congestion;

        let congestion: Arc<dyn congestion::ControllerFactory + Send + Sync + 'static> =
            match congestion {
                Congestion::Bbr => {
                    let mut config = congestion::BbrConfig::default();
                    if let Some(value) = initial_window {
                        config.initial_window(value);
                    }
                    Arc::new(config)
                }
                Congestion::Cubic => {
                    let mut config = congestion::CubicConfig::default();
                    if let Some(value) = initial_window {
                        config.initial_window(value);
                    }
                    Arc::new(config)
                }
                Congestion::NewReno => {
                    let mut config = congestion::NewRenoConfig::default();
                    if let Some(value) = initial_window {
                        config.initial_window(value);
                    }
                    Arc::new(config)
                }
            };

        self.transport_config
            .congestion_controller_factory(congestion);
        self
    }

    pub fn with_max_idle_timeout(&mut self, value: Duration) -> Result<&mut Self> {
        use quinn::IdleTimeout;
        self.transport_config
            .max_idle_timeout(Some(IdleTimeout::try_from(value)?));
        Ok(self)
    }

    pub fn with_max_keep_alive_period(&mut self, value: Duration) -> &mut Self {
        self.transport_config.keep_alive_interval(Some(value));
        self
    }

    pub fn with_max_open_bidirectional_streams(&mut self, value: u64) -> Result<&mut Self> {
        self.transport_config
            .max_concurrent_bidi_streams(VarInt::try_from(value)?);
        Ok(self)
    }

    pub async fn build(self) -> Result<QuicServer> {
        let server_config = self.build_server_config()?;
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

    fn build_server_config(&self) -> Result<ServerConfig> {
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

        let mut server_crypto = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)?;

        server_crypto.alpn_protocols = vec![b"h3".to_vec()];

        // Zero RTT
        if self.enable_zero_rtt {
            server_crypto.send_half_rtt_data = true;
            server_crypto.max_early_data_size = u32::MAX;
        }

        let server_config =
            ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(server_crypto)?));

        Ok(server_config)
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
    async fn accept_bidirectional(&self) -> io::Result<impl Reliable> {
        self.stream_receiver.recv().await.map_err(io::Error::other)
    }

    #[cfg(feature = "datagram")]
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
            biased;
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
            let remote_addr = connection.remote_address();
            debug!("QUIC Connection from {}", remote_addr);

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
        Err(e) => {
            error!("Failed to accept connection: {}", e)
        }
    }
}
