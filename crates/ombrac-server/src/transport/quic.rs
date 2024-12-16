use std::path::{Path, PathBuf};
use std::time::Duration;

use ombrac::Provider;
use tokio::sync::mpsc;

use crate::info;

use super::Result;

type Server = s2n_quic::server::Server;
type Stream = s2n_quic::stream::BidirectionalStream;

pub struct Quic(mpsc::Receiver<Stream>);

impl Provider for Quic {
    type Item = Stream;

    async fn fetch(&mut self) -> Option<Self::Item> {
        self.0.recv().await
    }
}

pub struct Builder {
    listen: String,

    tls_key: PathBuf,
    tls_cert: PathBuf,

    initial_congestion_window: Option<u32>,

    max_handshake_duration: Option<Duration>,
    max_idle_timeout: Option<Duration>,
    max_keep_alive_period: Option<Duration>,
    max_open_bidirectional_streams: Option<u64>,

    bidirectional_local_data_window: Option<u64>,
    bidirectional_remote_data_window: Option<u64>,
}

impl Builder {
    pub fn new(listen: String, tls_cert: PathBuf, tls_key: PathBuf) -> Self {
        Builder {
            listen,
            tls_cert,
            tls_key,
            initial_congestion_window: None,
            max_handshake_duration: None,
            max_idle_timeout: None,
            max_keep_alive_period: None,
            max_open_bidirectional_streams: None,
            bidirectional_local_data_window: None,
            bidirectional_remote_data_window: None,
        }
    }

    pub fn with_tls_cert(mut self, value: PathBuf) -> Self {
        self.tls_cert = value;
        self
    }

    pub fn with_tls_key(mut self, value: PathBuf) -> Self {
        self.tls_key = value;
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
}

impl Quic {
    async fn new(config: Builder) -> Result<Self> {
        let (sender, receiver) = mpsc::channel(1);

        let mut server = s2n_server_with_config(&config).await?;

        tokio::spawn(async move {
            while let Some(mut connection) = server.accept().await {
                if sender.is_closed() {
                    break;
                }

                let sender = sender.clone();

                tokio::spawn(async move {
                    info!(
                        "{:?} accept connection {} from {:?}",
                        connection.server_name(),
                        connection.id(),
                        connection.remote_addr()
                    );

                    while let Ok(Some(stream)) = connection.accept_bidirectional_stream().await {
                        if sender.send(stream).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });

        Ok(Self(receiver))
    }
}

async fn s2n_server_with_config(config: &Builder) -> Result<Server> {
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

    let server = Server::builder()
        .with_io(config.listen.as_str())?
        .with_limits(limits)?
        .with_congestion_controller(controller)?
        .with_tls((Path::new(&config.tls_cert), Path::new(&config.tls_key)))?
        .start()?;

    Ok(server)
}
