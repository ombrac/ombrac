use std::io::Result;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use std::{io, path::PathBuf};

use quinn::IdleTimeout;

use super::{stream::Stream, Connection};

pub struct Builder {
    listen: SocketAddr,

    tls_key: Option<PathBuf>,
    tls_cert: Option<PathBuf>,
    tls_skip: bool,

    enable_zero_rtt: bool,
    congestion_initial_window: Option<u64>,

    max_idle_timeout: Option<Duration>,
    max_keep_alive_period: Option<Duration>,
    max_open_bidirectional_streams: Option<u64>,
}

impl Builder {
    pub fn new<A>(addr: A) -> Self
    where
        A: Into<SocketAddr>,
    {
        Builder {
            listen: addr.into(),
            tls_cert: None,
            tls_key: None,
            tls_skip: false,
            enable_zero_rtt: false,
            congestion_initial_window: None,
            max_idle_timeout: None,
            max_keep_alive_period: None,
            max_open_bidirectional_streams: None,
        }
    }

    pub fn with_tls_cert(mut self, value: PathBuf) -> Self {
        self.tls_cert = Some(value);
        self
    }

    pub fn with_tls_key(mut self, value: PathBuf) -> Self {
        self.tls_key = Some(value);
        self
    }

    pub fn with_tls_skip(mut self, value: bool) -> Self {
        self.tls_skip = value;
        self
    }

    pub fn with_enable_zero_rtt(mut self, value: bool) -> Self {
        self.enable_zero_rtt = value;
        self
    }

    pub fn with_congestion_initial_window(mut self, value: u64) -> Self {
        self.congestion_initial_window = Some(value);
        self
    }

    pub fn with_max_idle_timeout(mut self, value: Duration) -> Self {
        self.max_idle_timeout = Some(value);
        self
    }

    pub fn with_max_keep_alive_period(mut self, value: Duration) -> Self {
        self.max_keep_alive_period = Some(value);
        self
    }

    pub fn with_max_open_bidirectional_streams(mut self, value: u64) -> Self {
        self.max_open_bidirectional_streams = Some(value);
        self
    }

    pub async fn build(self) -> Result<Connection> {
        Connection::with_server(self).await
    }
}

impl Connection {
    async fn with_server(config: Builder) -> Result<Self> {
        let tls_config = {
            use rustls::ServerConfig;

            use rustls::pki_types::CertificateDer;
            use rustls::pki_types::PrivatePkcs8KeyDer;

            let (cert, key) = if config.tls_skip {
                let signed = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
                let cert = vec![CertificateDer::from(signed.cert).into()];
                let key = PrivatePkcs8KeyDer::from(signed.key_pair.serialize_der()).into();
                (cert, key)
            } else {
                let (cert, key) = match (config.tls_cert, config.tls_key) {
                    (Some(cert_path), Some(key_path)) => {
                        let cert = super::load_certificates(&cert_path)?;
                        let key = super::load_private_key(&key_path)?;
                        (cert, key)
                    }
                    (Some(_), None) => {
                        return Err(io::Error::other(
                            "Private key must be provided when certificate is specified",
                        ))
                    }
                    (None, _) => {
                        return Err(io::Error::other(
                            "Certificate must be provided when TLS is enabled",
                        ))
                    }
                };
                (cert, key)
            };

            let mut tls_config = ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(cert, key)
                .map_err(|e| io::Error::other(e.to_string()))?;

            tls_config.alpn_protocols = [b"h3"].iter().map(|&x| x.into()).collect();

            if config.enable_zero_rtt {
                tls_config.send_half_rtt_data = true;
                tls_config.max_early_data_size = u32::MAX;
            }

            tls_config
        };

        let quic_config = {
            use quinn::crypto::rustls::QuicServerConfig;

            let config = QuicServerConfig::try_from(tls_config)
                .map_err(|e| io::Error::other(e.to_string()))?;

            config
        };

        let server_config = {
            use quinn::{congestion, ServerConfig, TransportConfig, VarInt};

            let mut transport = TransportConfig::default();
            let mut congestion = congestion::BbrConfig::default();

            if let Some(value) = config.congestion_initial_window {
                congestion.initial_window(value);
            }

            if let Some(value) = config.max_idle_timeout {
                transport.max_idle_timeout(Some(
                    IdleTimeout::try_from(value).map_err(|e| io::Error::other(e.to_string()))?,
                ));
            }

            if let Some(value) = config.max_keep_alive_period {
                transport.keep_alive_interval(Some(value));
            }

            if let Some(value) = config.max_open_bidirectional_streams {
                transport.max_concurrent_bidi_streams(
                    VarInt::from_u64(value).map_err(|e| io::Error::other(e.to_string()))?,
                );
            }

            transport.congestion_controller_factory(Arc::new(congestion));

            let mut config = ServerConfig::with_crypto(Arc::new(quic_config));
            config.transport_config(Arc::new(transport));

            config
        };

        let endpoint = {
            use quinn::Endpoint;

            Endpoint::server(server_config, config.listen)?
        };

        let (sender, receiver) = async_channel::unbounded();

        #[cfg(feature = "datagram")]
        let (datagram_sender, datagram_receiver) = async_channel::unbounded();

        let handle = tokio::spawn(async move {
            while let Some(connecting) = endpoint.accept().await {
                let sender = sender.clone();

                #[cfg(feature = "datagram")]
                let datagram_sender = datagram_sender.clone();

                tokio::spawn(async move {
                    let connection = match connecting.await {
                        Ok(conn) => conn,
                        Err(_) => return,
                    };

                    #[cfg(feature = "datagram")]
                    {
                        use crate::quic::datagram::Session;

                        let conn = connection.clone();
                        tokio::spawn(async move {
                            let session = Session::with_server(conn);

                            while let Some(datagram) = session.accept_bidirectional().await {
                                if datagram_sender.send(datagram).await.is_err() {
                                    break;
                                }
                            }
                        });
                    }

                    while let Ok((send_stream, recv_stream)) = connection.accept_bi().await {
                        if sender.send(Stream(send_stream, recv_stream)).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });

        Ok(Connection {
            handle,
            stream: receiver,
            #[cfg(feature = "datagram")]
            datagram: datagram_receiver,
        })
    }
}
