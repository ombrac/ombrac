use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use ombrac::Provider;
use tokio::sync::mpsc;

use crate::{error, info};

use super::Result;

type Client = s2n_quic::client::Client;
type Stream = s2n_quic::stream::BidirectionalStream;

pub struct Quic(mpsc::Receiver<Stream>);

impl Provider for Quic {
    type Item = Stream;

    fn fetch(&mut self) -> impl Future<Output = Option<Self::Item>> {
        self.0.recv()
    }
}

pub struct Builder {
    bind: Option<String>,

    server_name: Option<String>,
    server_address: String,

    tls_cert: Option<PathBuf>,

    initial_congestion_window: Option<u32>,

    max_handshake_duration: Option<Duration>,
    max_idle_timeout: Option<Duration>,
    max_keep_alive_period: Option<Duration>,
    max_open_bidirectional_streams: Option<u64>,

    bidirectional_local_data_window: Option<u64>,
    bidirectional_remote_data_window: Option<u64>,
}

impl Builder {
    pub fn new(server_address: String) -> Self {
        Builder {
            bind: None,
            server_name: None,
            server_address,
            tls_cert: None,
            initial_congestion_window: None,
            max_handshake_duration: None,
            max_idle_timeout: None,
            max_keep_alive_period: None,
            max_open_bidirectional_streams: None,
            bidirectional_local_data_window: None,
            bidirectional_remote_data_window: None,
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

    pub fn with_initial_congestion_window(mut self, value: u32) -> Self {
        self.initial_congestion_window = Some(value);
        self
    }

    pub fn with_max_handshake_duration(mut self, value: Duration) -> Self {
        self.max_handshake_duration = Some(value);
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

    pub fn with_bidirectional_local_data_window(mut self, value: u64) -> Self {
        self.bidirectional_local_data_window = Some(value);
        self
    }

    pub fn with_bidirectional_remote_data_window(mut self, value: u64) -> Self {
        self.bidirectional_remote_data_window = Some(value);
        self
    }

    pub async fn build(self) -> Result<Quic> {
        Quic::new(self).await
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

    async fn server_address(&self) -> Result<SocketAddr> {
        use tokio::net::lookup_host;

        let address = lookup_host(&self.server_address)
            .await?
            .next()
            .ok_or(format!(
                "unable to resolve address '{}'",
                self.server_address
            ))?;

        Ok(address)
    }
}

impl Quic {
    async fn new(config: Builder) -> Result<Self> {
        use s2n_quic::client::Connect;

        let client = s2n_client_with_config(&config).await?;
        let server_addr = config.server_address().await?;
        let server_name = config.server_name()?.to_string();
        let connect = Connect::new(server_addr).with_server_name(server_name);

        let (sender, receiver) = mpsc::channel(1);

        tokio::spawn(async move {
            'connection: loop {
                let mut connection = match client.connect(connect.clone()).await {
                    Ok(value) => value,
                    Err(_error) => {
                        error!(
                            "failed to establish connection with {}. {}",
                            server_addr, _error
                        );

                        continue 'connection;
                    }
                };

                if let Err(_error) = connection.keep_alive(true) {
                    error!(
                        "failed to keep alive the connection {}. {}",
                        connection.id(),
                        _error
                    );

                    continue 'connection;
                };

                info!(
                    "{:?} establish connection {} with {:?}",
                    connection.local_addr(),
                    connection.id(),
                    connection.remote_addr()
                );

                'stream: loop {
                    let stream = match connection.open_bidirectional_stream().await {
                        Ok(stream) => stream,
                        Err(_error) => {
                            error!(
                                "connection {} failed to open bidirectional stream. {}",
                                connection.id(),
                                _error
                            );

                            break 'stream;
                        }
                    };

                    if sender.send(stream).await.is_err() {
                        break 'connection;
                    }
                }
            }
        });

        Ok(Self(receiver))
    }
}

async fn s2n_client_with_config(config: &Builder) -> Result<Client> {
    use s2n_quic::provider::congestion_controller;
    use s2n_quic::provider::limits;

    let limits = {
        let mut limits = limits::Limits::new();

        if let Some(value) = config.bidirectional_local_data_window {
            limits = limits.with_bidirectional_local_data_window(value)?;
        }

        if let Some(value) = config.bidirectional_remote_data_window {
            limits = limits.with_bidirectional_remote_data_window(value)?;
        }

        if let Some(value) = config.max_open_bidirectional_streams {
            limits = limits.with_max_open_local_bidirectional_streams(value)?;
        }

        if let Some(value) = config.max_open_bidirectional_streams {
            limits = limits.with_max_open_remote_bidirectional_streams(value)?;
        }

        if let Some(value) = config.max_handshake_duration {
            limits = limits.with_max_handshake_duration(value)?;
        }

        if let Some(value) = config.max_keep_alive_period {
            limits = limits.with_max_keep_alive_period(value)?;
        }

        if let Some(value) = config.max_idle_timeout {
            limits = limits.with_max_idle_timeout(value)?;
        }

        limits
    };

    let controller = {
        let mut controller = congestion_controller::bbr::Builder::default();

        if let Some(value) = config.initial_congestion_window {
            controller = controller.with_initial_congestion_window(value);
        }

        controller.build()
    };

    let io = match &config.bind {
        Some(value) => value,
        None => match config.server_address().await? {
            SocketAddr::V4(_) => "0.0.0.0:0",
            SocketAddr::V6(_) => "[::]:0",
        },
    };

    let client = Client::builder()
        .with_io(io)?
        .with_limits(limits)?
        .with_congestion_controller(controller)?;

    let client = match &config.tls_cert {
        Some(path) => client.with_tls(Path::new(path))?.start()?,
        None => client.start()?,
    };

    Ok(client)
}
