use std::io;
use std::net::UdpSocket;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use ombrac_macros::{error, info, warn};
use ombrac_transport::quic::TransportConfig as QuicTransportConfig;
use ombrac_transport::quic::error::Error as QuicError;
use ombrac_transport::quic::server::Config as QuicConfig;
use ombrac_transport::quic::server::Server as QuicServer;

use crate::config::{ServiceConfig, TlsMode};
use crate::connection::ConnectionAcceptor;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),

    #[error("{0}")]
    Config(String),

    #[error(transparent)]
    Quic(#[from] QuicError),
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

/// OmbracServer provides a simple, easy-to-use API for starting and managing
/// the ombrac server using QUIC transport.
///
/// This struct hides all transport-specific implementation details and provides
/// a clean interface for external users.
///
/// # Example
///
/// ```no_run
/// use ombrac_server::{OmbracServer, ServiceConfig};
/// use std::sync::Arc;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let config = Arc::new(ServiceConfig {
///     secret: "my-secret".to_string(),
///     listen: "0.0.0.0:8080".parse()?,
///     transport: Default::default(),
///     connection: Default::default(),
///     logging: Default::default(),
/// });
///
/// let server = OmbracServer::build(config).await?;
/// // ... use server ...
/// server.shutdown().await;
/// # Ok(())
/// # }
/// ```
pub struct OmbracServer {
    handle: JoinHandle<Result<()>>,
    shutdown_tx: broadcast::Sender<()>,
}

impl OmbracServer {
    /// Builds a new server instance from the configuration.
    ///
    /// This method:
    /// 1. Creates a QUIC server from the transport configuration
    /// 2. Sets up connection validation using the secret
    /// 3. Spawns the accept loop in a background task
    /// 4. Returns an OmbracServer handle for lifecycle management
    ///
    /// # Arguments
    ///
    /// * `config` - The service configuration containing transport, connection, and secret settings
    ///
    /// # Returns
    ///
    /// A configured `OmbracServer` instance ready to accept connections, or an error
    /// if configuration is invalid or server setup fails.
    pub async fn build(config: Arc<ServiceConfig>) -> Result<Self> {
        // Build QUIC server from config
        let acceptor = quic_server_from_config(&config).await?;

        // Create secret authenticator from config
        let secret = *blake3::hash(config.secret.as_bytes()).as_bytes();

        // Create connection acceptor with connection config
        let connection_config = Arc::new(config.connection.clone());
        let acceptor = Arc::new(ConnectionAcceptor::with_config(
            acceptor,
            secret,
            connection_config,
        ));

        // Set up shutdown channel
        let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

        // Spawn accept loop
        let handle =
            tokio::spawn(async move { acceptor.accept_loop(shutdown_rx).await.map_err(Error::Io) });

        Ok(OmbracServer {
            handle,
            shutdown_tx,
        })
    }

    /// Gracefully shuts down the server.
    ///
    /// This method will:
    /// 1. Send a shutdown signal to stop accepting new connections
    /// 2. Wait for the accept loop to finish gracefully
    /// 3. Wait for existing connections to close
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use ombrac_server::OmbracServer;
    /// # use std::sync::Arc;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let config = Arc::new(ombrac_server::ServiceConfig {
    /// #     secret: "test".to_string(),
    /// #     listen: "0.0.0.0:0".parse()?,
    /// #     transport: Default::default(),
    /// #     connection: Default::default(),
    /// #     logging: Default::default(),
    /// # });
    /// # let server = OmbracServer::build(config).await?;
    /// server.shutdown().await;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn shutdown(self) {
        let _ = self.shutdown_tx.send(());
        match self.handle.await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                error!("the main server task exited with an error: {e}");
            }
            Err(e) => {
                error!("the main server task failed to shut down cleanly: {e}");
            }
        }
    }
}

async fn quic_server_from_config(config: &ServiceConfig) -> Result<QuicServer> {
    let transport_cfg = &config.transport;
    let mut quic_config = QuicConfig::new();

    quic_config.enable_zero_rtt = transport_cfg.zero_rtt();
    quic_config.alpn_protocols = transport_cfg.alpn_protocols();

    match transport_cfg.tls_mode() {
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
            warn!("tls is in insecure mode; generating self-signed certificates for local/dev use");
            quic_config.enable_self_signed = true;
        }
    }

    let mut transport_config = QuicTransportConfig::default();
    let map_transport_err = |e: QuicError| Error::Quic(e);
    transport_config
        .max_idle_timeout(Duration::from_millis(transport_cfg.idle_timeout()))
        .map_err(map_transport_err)?;
    transport_config
        .keep_alive_period(Duration::from_millis(transport_cfg.keep_alive()))
        .map_err(map_transport_err)?;
    transport_config
        .max_open_bidirectional_streams(transport_cfg.max_streams())
        .map_err(map_transport_err)?;
    transport_config
        .congestion(transport_cfg.congestion(), transport_cfg.cwnd_init)
        .map_err(map_transport_err)?;
    quic_config.transport_config(transport_config);

    info!("binding udp socket to {}", config.listen);
    let socket = UdpSocket::bind(config.listen)?;

    QuicServer::new(socket, quic_config)
        .await
        .map_err(|e| Error::Quic(e))
}
