use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use super::{Connection, Result, Stream};

pub struct Builder {
    bind: Option<String>,

    server_name: Option<String>,
    server_address: String,

    tls_cert: Option<PathBuf>,

    enable_zero_rtt: bool,
    enable_connection_multiplexing: bool,
    congestion_initial_window: Option<u64>,

    max_idle_timeout: Option<Duration>,
    max_keep_alive_period: Option<Duration>,
    max_open_bidirectional_streams: Option<u64>,
}

impl Builder {
    pub fn new(server_address: String) -> Self {
        Builder {
            bind: None,
            server_name: None,
            server_address,
            tls_cert: None,
            enable_zero_rtt: false,
            enable_connection_multiplexing: false,
            congestion_initial_window: None,
            max_idle_timeout: None,
            max_keep_alive_period: None,
            max_open_bidirectional_streams: None,
        }
    }

    pub fn with_server_name(mut self, value: String) -> Self {
        self.server_name = Some(value);
        self
    }

    pub fn with_bind(mut self, value: String) -> Self {
        self.bind = Some(value);
        self
    }

    pub fn with_tls_cert(mut self, value: PathBuf) -> Self {
        self.tls_cert = Some(value);
        self
    }

    pub fn with_enable_zero_rtt(mut self, value: bool) -> Self {
        self.enable_zero_rtt = value;
        self
    }

    pub fn with_enable_connection_multiplexing(mut self, value: bool) -> Self {
        self.enable_connection_multiplexing = value;
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
        Connection::with_client(self).await
    }

    fn server_name(&self) -> Result<&str> {
        match &self.server_name {
            Some(value) => Ok(value),
            None => {
                let pos = self
                    .server_address
                    .rfind(':')
                    .ok_or(format!("invalid server address {}", self.server_address))?;

                Ok(&self.server_address[..pos])
            }
        }
    }

    async fn bind_address(&self) -> Result<SocketAddr> {
        use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6};

        let address = match &self.bind {
            Some(value) => value.parse()?,
            None => match self.server_address().await? {
                SocketAddr::V4(_) => SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)),
                SocketAddr::V6(_) => {
                    SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0))
                }
            },
        };

        Ok(address)
    }

    async fn server_address(&self) -> Result<SocketAddr> {
        use tokio::net::lookup_host;

        let address = lookup_host(&self.server_address)
            .await?
            .next()
            .ok_or(format!(
                "failed to resolve server address '{}'",
                self.server_address
            ))?;

        Ok(address)
    }
}

impl Connection {
    async fn with_client(config: Builder) -> Result<Self> {
        let tls_config = {
            use rustls::{ClientConfig, RootCertStore};

            let mut roots = RootCertStore::empty();

            if let Some(path) = &config.tls_cert {
                let certs = super::load_certificates(path)?;
                roots.add_parsable_certificates(certs);
            } else {
                roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            }

            let mut tls_config = ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth();

            tls_config.alpn_protocols = [b"h3"].iter().map(|&x| x.into()).collect();

            if config.enable_zero_rtt {
                tls_config.enable_early_data = true;
            }

            tls_config
        };

        let quic_config = {
            use quinn::crypto::rustls::QuicClientConfig;

            let config = QuicClientConfig::try_from(tls_config)?;

            config
        };

        let client_config = {
            use quinn::{congestion, ClientConfig, TransportConfig, VarInt};

            let mut transport = TransportConfig::default();
            let mut congestion = congestion::BbrConfig::default();

            if let Some(value) = config.congestion_initial_window {
                congestion.initial_window(value);
            }

            if let Some(value) = config.max_idle_timeout {
                transport.max_idle_timeout(Some(value.try_into()?));
            }

            if let Some(value) = config.max_keep_alive_period {
                transport.keep_alive_interval(Some(value));
            }

            if let Some(value) = config.max_open_bidirectional_streams {
                transport.max_concurrent_bidi_streams(VarInt::from_u64(value)?);
            }

            transport.congestion_controller_factory(Arc::new(congestion));

            let mut config = ClientConfig::new(Arc::new(quic_config));
            config.transport_config(Arc::new(transport));

            config
        };

        let endpoint = {
            use quinn::Endpoint;

            let bind_address = config.bind_address().await?;

            let mut endpoint = Endpoint::client(bind_address)?;
            endpoint.set_default_client_config(client_config);

            endpoint
        };

        let server_name = config.server_name()?.to_string();
        let server_address = config.server_address().await?;

        let enable_zero_rtt = config.enable_zero_rtt;
        let enable_connection_multiplexing = config.enable_connection_multiplexing;

        let (sender, receiver) = async_channel::bounded(1);

        tokio::spawn(async move {
            use ombrac_macros::{try_or_break, try_or_continue};

            'connection: loop {
                let connection = try_or_continue!(endpoint.connect(server_address, &server_name));

                let connection = if enable_zero_rtt {
                    match connection.into_0rtt() {
                        Ok((conn, zero_rtt_accepted)) => {
                            if !zero_rtt_accepted.await {
                                continue 'connection;
                            };

                            conn
                        }
                        Err(conn) => {
                            try_or_continue!(conn.await)
                        }
                    }
                } else {
                    try_or_continue!(connection.await)
                };

                let sender = sender.clone();

                'stream: loop {
                    let stream = try_or_break!(connection.open_bi().await);

                    if sender.send(Stream(stream.0, stream.1)).await.is_err() {
                        break 'connection;
                    }

                    if !enable_connection_multiplexing {
                        break 'stream;
                    }
                }
            }
        });

        Ok(Connection(receiver))
    }
}
