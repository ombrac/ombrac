use std::io;
use std::net::UdpSocket;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use ombrac::metrics::Metrics;
use ombrac::protocol::Secret;
use ombrac_macros::{error, info, warn};
use ombrac_transport::quic::TransportConfig as QuicTransportConfig;
use ombrac_transport::quic::error::Error as QuicError;
use ombrac_transport::quic::server::Config as QuicConfig;
use ombrac_transport::quic::server::Server as QuicServer;

use crate::config::{ServiceConfig, TlsMode};
use crate::connection::ConnectionAcceptor;

type BuiltAcceptor = ConnectionAcceptor<QuicServer, Secret>;

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
    metrics: Metrics,
    // Held to keep the QUIC endpoint alive after the accept loop exits, so
    // `shutdown_with_drain` can wait for in-flight streams without the
    // underlying transport being torn down.
    _acceptor_keepalive: Arc<BuiltAcceptor>,
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
        let metrics = acceptor.metrics();

        // Set up shutdown channel
        let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

        // Spawn accept loop. Hold a strong reference to the acceptor (and thus
        // the underlying QUIC server) outside the task as well, so the
        // transport stays alive after the accept loop exits — needed for
        // shutdown_with_drain to let in-flight streams finish naturally.
        let acceptor_for_task = Arc::clone(&acceptor);
        let handle = tokio::spawn(async move {
            acceptor_for_task
                .accept_loop(shutdown_rx)
                .await
                .map_err(Error::Io)
        });

        Ok(OmbracServer {
            handle,
            shutdown_tx,
            metrics,
            _acceptor_keepalive: acceptor,
        })
    }

    /// Returns a clone-able handle to runtime metrics for this server.
    ///
    /// Callers can snapshot or read individual counters at any time:
    /// `server.metrics().snapshot()`.
    pub fn metrics(&self) -> Metrics {
        self.metrics.clone()
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

    /// Stops accepting new connections, then waits up to `drain_timeout` for
    /// in-flight streams to close naturally before returning.
    ///
    /// Returns `true` if all streams drained within the timeout, `false` if
    /// the timeout elapsed with active streams still in flight. In the latter
    /// case, those streams' tasks will keep running until their underlying
    /// QUIC connection closes (typically via idle timeout) — they are not
    /// hard-cancelled by this call.
    ///
    /// Use this for rolling restarts where you want existing client requests
    /// to complete before the process exits.
    pub async fn shutdown_with_drain(self, drain_timeout: Duration) -> bool {
        let _ = self.shutdown_tx.send(());

        // Poll the stream counter until balanced (no active streams) or timeout.
        let deadline = tokio::time::Instant::now() + drain_timeout;
        let mut interval = tokio::time::interval(Duration::from_millis(50));
        let drained = loop {
            interval.tick().await;
            let snap = self.metrics.snapshot();
            let active_streams = snap.streams_opened.saturating_sub(snap.streams_closed);
            if active_streams == 0 {
                break true;
            }
            if tokio::time::Instant::now() >= deadline {
                warn!(
                    active_streams = active_streams,
                    timeout_ms = drain_timeout.as_millis() as u64,
                    "drain timed out with streams still active"
                );
                break false;
            }
        };

        // Either way, wait for the accept-loop task to clean up.
        match self.handle.await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => error!("server task exited with error: {e}"),
            Err(e) => error!("server task failed to shut down cleanly: {e}"),
        }

        drained
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
            warn!(
                "================================================================"
            );
            warn!(
                "TLS IS IN INSECURE MODE — self-signed certificates will be used."
            );
            warn!(
                "This bypasses any meaningful authentication of the SERVER and"
            );
            warn!(
                "is intended for local development and CI only. DO NOT use this"
            );
            warn!(
                "mode on a network you don't control."
            );
            warn!(
                "================================================================"
            );
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
        .map_err(Error::Quic)
}
