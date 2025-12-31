use std::future::Future;
use std::io;
use std::marker::PhantomData;
use std::net::UdpSocket;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use ombrac_macros::{error, info, warn};
use ombrac_transport::quic::{
    self, Connection as QuicConnection, TransportConfig as QuicTransportConfig,
    server::{Config as QuicConfig, Server as QuicServer},
};
use ombrac_transport::{Acceptor, Connection};

use crate::config::{ServiceConfig, TlsMode};
use crate::connection::ConnectionHandler;
use crate::server::Server;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("{0}")]
    Io(#[from] io::Error),

    #[error("Transport layer error: {0}")]
    Transport(String),
}

pub type Result<T> = std::result::Result<T, Error>;

macro_rules! require_config {
    ($config_opt:expr, $field_name:expr) => {
        $config_opt.ok_or_else(|| {
            Error::Config(format!(
                "'{}' is required but was not provided",
                $field_name
            ))
        })
    };
}

pub trait ServiceBuilder {
    type Acceptor: Acceptor<Connection = Self::Connection>;
    type Connection: Connection;
    type Validator: ConnectionHandler<Self::Connection> + 'static;

    fn build(
        config: &Arc<ServiceConfig>,
    ) -> impl Future<Output = Result<Arc<Server<Self::Acceptor, Self::Validator>>>> + Send;
}

pub struct QuicServiceBuilder;

impl ServiceBuilder for QuicServiceBuilder {
    type Acceptor = QuicServer;
    type Connection = QuicConnection;
    type Validator = ombrac::protocol::Secret;

    async fn build(
        config: &Arc<ServiceConfig>,
    ) -> Result<Arc<Server<Self::Acceptor, Self::Validator>>> {
        let acceptor = quic_server_from_config(config).await?;
        let secret = *blake3::hash(config.secret.as_bytes()).as_bytes();
        let server = Arc::new(Server::new(acceptor, secret));
        Ok(server)
    }
}

pub struct Service<T, C>
where
    T: Acceptor<Connection = C>,
    C: Connection,
{
    handle: JoinHandle<Result<()>>,
    shutdown_tx: broadcast::Sender<()>,
    _acceptor: PhantomData<T>,
    _connection: PhantomData<C>,
}

impl<T, C> Service<T, C>
where
    T: Acceptor<Connection = C> + Send + Sync + 'static,
    C: Connection + Send + Sync + 'static,
{
    pub async fn build<Builder, V>(config: Arc<ServiceConfig>) -> Result<Self>
    where
        Builder: ServiceBuilder<Acceptor = T, Connection = C, Validator = V>,
        V: ConnectionHandler<C> + Send + Sync + 'static,
    {
        let server = Builder::build(&config).await?;
        let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

        let handle =
            tokio::spawn(async move { server.accept_loop(shutdown_rx).await.map_err(Error::Io) });

        Ok(Service {
            handle,
            shutdown_tx,
            _acceptor: PhantomData,
            _connection: PhantomData,
        })
    }

    pub async fn shutdown(self) {
        let _ = self.shutdown_tx.send(());
        match self.handle.await {
            Ok(Ok(_)) => {}
            Ok(Err(_e)) => {
                error!("The main server task exited with an error: {_e}");
            }
            Err(_e) => {
                error!("The main server task failed to shut down cleanly: {_e}");
            }
        }
        warn!("Service shutdown complete");
    }
}

async fn quic_server_from_config(config: &ServiceConfig) -> Result<QuicServer> {
    let transport_cfg = &config.transport;
    let mut quic_config = QuicConfig::new();

    quic_config.enable_zero_rtt = transport_cfg.zero_rtt.unwrap_or(false);
    if let Some(protocols) = &transport_cfg.alpn_protocols {
        quic_config.alpn_protocols = protocols.clone();
    }

    match transport_cfg.tls_mode.unwrap_or(TlsMode::Tls) {
        TlsMode::Tls => {
            let cert_path = require_config!(transport_cfg.tls_cert.clone(), "transport.tls_cert")?;
            let key_path = require_config!(transport_cfg.tls_key.clone(), "transport.tls_key")?;
            quic_config.tls_cert_key_paths = Some((cert_path, key_path));
        }
        TlsMode::MTls => {
            let cert_path = require_config!(
                transport_cfg.tls_cert.clone(),
                "transport.tls_cert for mTLS"
            )?;
            let key_path =
                require_config!(transport_cfg.tls_key.clone(), "transport.tls_key for mTLS")?;
            quic_config.tls_cert_key_paths = Some((cert_path, key_path));
            quic_config.root_ca_path = Some(require_config!(
                transport_cfg.ca_cert.clone(),
                "transport.ca_cert for mTLS"
            )?);
        }
        TlsMode::Insecure => {
            warn!("TLS is running in insecure mode. Self-signed certificates will be generated.");
            quic_config.enable_self_signed = true;
        }
    }

    let mut transport_config = QuicTransportConfig::default();
    let map_transport_err = |e: quic::error::Error| Error::Transport(e.to_string());
    if let Some(timeout) = transport_cfg.idle_timeout {
        transport_config
            .max_idle_timeout(Duration::from_millis(timeout))
            .map_err(map_transport_err)?;
    }
    if let Some(interval) = transport_cfg.keep_alive {
        transport_config
            .keep_alive_period(Duration::from_millis(interval))
            .map_err(map_transport_err)?;
    }
    if let Some(max_streams) = transport_cfg.max_streams {
        transport_config
            .max_open_bidirectional_streams(max_streams)
            .map_err(map_transport_err)?;
    }
    if let Some(congestion) = transport_cfg.congestion {
        transport_config
            .congestion(congestion, transport_cfg.cwnd_init)
            .map_err(map_transport_err)?;
    }
    quic_config.transport_config(transport_config);

    info!("Binding UDP socket to {}", config.listen);
    let socket = UdpSocket::bind(config.listen)?;

    QuicServer::new(socket, quic_config)
        .await
        .map_err(|e| Error::Transport(e.to_string()))
}
