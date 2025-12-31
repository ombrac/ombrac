use std::io;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use ombrac_macros::{error, info, warn};
use ombrac_transport::quic::Connection as QuicConnection;
use ombrac_transport::quic::TransportConfig as QuicTransportConfig;
use ombrac_transport::quic::client::Client as QuicClient;
use ombrac_transport::quic::client::Config as QuicConfig;
use ombrac_transport::quic::error::Error as QuicError;

use crate::client::Client;
use crate::config::{ServiceConfig, TlsMode};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),

    #[error("{0}")]
    Config(String),

    #[error(transparent)]
    Quic(#[from] QuicError),

    #[error("{0}")]
    Endpoint(String),
}

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

/// OmbracClient provides a simple, easy-to-use API for starting and managing
/// the ombrac client using QUIC transport.
///
/// This struct hides all transport-specific implementation details and provides
/// a clean interface for external users.
///
/// # Example
///
/// ```no_run
/// use ombrac_client::{OmbracClient, ServiceConfig};
/// use std::sync::Arc;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let config = Arc::new(ServiceConfig {
///     secret: "my-secret".to_string(),
///     server: "server.example.com:8080".to_string(),
///     handshake_option: None,
///     endpoint: Default::default(),
///     transport: Default::default(),
///     logging: Default::default(),
/// });
///
/// let client = OmbracClient::build(config).await?;
/// // ... use client ...
/// client.shutdown().await;
/// # Ok(())
/// # }
/// ```
pub struct OmbracClient {
    client: Arc<Client<QuicClient, QuicConnection>>,
    handles: Vec<JoinHandle<()>>,
    shutdown_tx: broadcast::Sender<()>,
}

impl OmbracClient {
    /// Builds a new client instance from the configuration.
    ///
    /// This method:
    /// 1. Creates a QUIC client from the transport configuration
    /// 2. Establishes connection with handshake
    /// 3. Spawns endpoint tasks if configured
    /// 4. Returns an OmbracClient handle for lifecycle management
    ///
    /// # Arguments
    ///
    /// * `config` - The service configuration containing transport, endpoint, and secret settings
    ///
    /// # Returns
    ///
    /// A configured `OmbracClient` instance ready to use, or an error
    /// if configuration is invalid or client setup fails.
    pub async fn build(config: Arc<ServiceConfig>) -> Result<Self> {
        // Build QUIC client from config
        let transport = quic_client_from_config(&config).await?;
        let secret = *blake3::hash(config.secret.as_bytes()).as_bytes();
        let client = Arc::new(
            Client::new(
                transport,
                secret,
                config.handshake_option.clone().map(Into::into),
            )
            .await
            .map_err(|e| Error::Io(e))?,
        );

        let mut handles = Vec::new();
        let (shutdown_tx, _) = broadcast::channel(1);

        // Start HTTP endpoint if configured
        #[cfg(feature = "endpoint-http")]
        if config.endpoint.http.is_some() {
            handles.push(Self::spawn_endpoint(
                "HTTP",
                Self::endpoint_http_accept_loop(
                    config.clone(),
                    client.clone(),
                    shutdown_tx.subscribe(),
                ),
            ));
        }

        // Start SOCKS endpoint if configured
        #[cfg(feature = "endpoint-socks")]
        if config.endpoint.socks.is_some() {
            handles.push(Self::spawn_endpoint(
                "SOCKS",
                Self::endpoint_socks_accept_loop(
                    config.clone(),
                    client.clone(),
                    shutdown_tx.subscribe(),
                ),
            ));
        }

        // Start TUN endpoint if configured
        #[cfg(feature = "endpoint-tun")]
        if let Some(tun_config) = &config.endpoint.tun
            && (tun_config.tun_ipv4.is_some()
                || tun_config.tun_ipv6.is_some()
                || tun_config.tun_fd.is_some())
        {
            handles.push(Self::spawn_endpoint(
                "TUN",
                Self::endpoint_tun_accept_loop(
                    config.clone(),
                    client.clone(),
                    shutdown_tx.subscribe(),
                ),
            ));
        }

        if handles.is_empty() {
            return Err(Error::Config(
                "no endpoints were configured or enabled, the service has nothing to do."
                    .to_string(),
            ));
        }

        Ok(OmbracClient {
            client,
            handles,
            shutdown_tx,
        })
    }

    /// Rebind the transport to a new socket to ensure a clean state for reconnection.
    pub async fn rebind(&self) -> io::Result<()> {
        self.client.rebind().await
    }

    /// Gracefully shuts down the client.
    ///
    /// This method will:
    /// 1. Send a shutdown signal to stop accepting new connections
    /// 2. Wait for all endpoint tasks to finish gracefully
    /// 3. Close existing connections
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use ombrac_client::OmbracClient;
    /// # use std::sync::Arc;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let config = Arc::new(ombrac_client::ServiceConfig {
    /// #     secret: "test".to_string(),
    /// #     server: "server:8080".to_string(),
    /// #     handshake_option: None,
    /// #     endpoint: Default::default(),
    /// #     transport: Default::default(),
    /// #     logging: Default::default(),
    /// # });
    /// # let client = OmbracClient::build(config).await?;
    /// client.shutdown().await;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn shutdown(self) {
        let _ = self.shutdown_tx.send(());

        for handle in self.handles {
            if let Err(_err) = handle.await {
                error!("task failed to shut down cleanly: {:?}", _err);
            }
        }
        warn!("service shutdown complete");
    }

    fn spawn_endpoint(
        _name: &'static str,
        task: impl std::future::Future<Output = Result<()>> + Send + 'static,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            if let Err(_e) = task.await {
                error!("the {_name} endpoint shut down due to an error: {_e}");
            }
        })
    }

    #[cfg(feature = "endpoint-http")]
    async fn endpoint_http_accept_loop(
        config: Arc<ServiceConfig>,
        ombrac: Arc<Client<QuicClient, QuicConnection>>,
        mut shutdown_rx: broadcast::Receiver<()>,
    ) -> Result<()> {
        use crate::endpoint::http::Server as HttpServer;
        let bind_addr = require_config!(config.endpoint.http, "endpoint.http")?;
        let socket = tokio::net::TcpListener::bind(bind_addr).await?;

        info!("starting http/https endpoint, listening on {bind_addr}");

        HttpServer::new(ombrac)
            .accept_loop(socket, async {
                let _ = shutdown_rx.recv().await;
            })
            .await
            .map_err(|e| Error::Endpoint(format!("http server failed to run: {}", e)))
    }

    #[cfg(feature = "endpoint-socks")]
    async fn endpoint_socks_accept_loop(
        config: Arc<ServiceConfig>,
        ombrac: Arc<Client<QuicClient, QuicConnection>>,
        mut shutdown_rx: broadcast::Receiver<()>,
    ) -> Result<()> {
        use crate::endpoint::socks::CommandHandler;
        use socks_lib::v5::server::auth::NoAuthentication;
        use socks_lib::v5::server::{Config as SocksConfig, Server as SocksServer};

        let bind_addr = require_config!(config.endpoint.socks, "endpoint.socks")?;
        let socket = tokio::net::TcpListener::bind(bind_addr).await?;

        info!("starting socks endpoint, listening on {bind_addr}");

        let socks_config = Arc::new(SocksConfig::new(
            NoAuthentication,
            CommandHandler::new(ombrac),
        ));
        SocksServer::run(socket, socks_config, async {
            let _ = shutdown_rx.recv().await;
        })
        .await
        .map_err(|e| Error::Endpoint(format!("socks server failed to run: {}", e)))
    }

    #[cfg(feature = "endpoint-tun")]
    async fn endpoint_tun_accept_loop(
        config: Arc<ServiceConfig>,
        ombrac: Arc<Client<QuicClient, QuicConnection>>,
        mut shutdown_rx: broadcast::Receiver<()>,
    ) -> Result<()> {
        use crate::endpoint::tun::{AsyncDevice, Tun, TunConfig};

        let config = require_config!(config.endpoint.tun.as_ref(), "endpoint.tun")?;

        let device = match config.tun_fd {
            Some(fd) => {
                #[cfg(not(windows))]
                unsafe {
                    AsyncDevice::from_fd(fd)?
                }

                #[cfg(windows)]
                return Err(Error::Config(
                    "'tun_fd' option is not supported on Windows.".to_string(),
                ));
            }
            None => {
                #[cfg(not(any(target_os = "android", target_os = "ios")))]
                {
                    let device = {
                        use std::str::FromStr;

                        let mut builder = tun_rs::DeviceBuilder::new();
                        builder = builder.mtu(config.tun_mtu.unwrap_or(1500));

                        if let Some(ip_str) = &config.tun_ipv4 {
                            let ip = ipnet::Ipv4Net::from_str(ip_str).map_err(|e| {
                                Error::Config(format!("failed to parse ipv4 cidr '{ip_str}': {e}"))
                            })?;
                            builder = builder.ipv4(ip.addr(), ip.netmask(), None);
                        }

                        if let Some(ip_str) = &config.tun_ipv6 {
                            let ip = ipnet::Ipv6Net::from_str(ip_str).map_err(|e| {
                                Error::Config(format!("failed to parse ipv6 cidr '{ip_str}': {e}"))
                            })?;
                            builder = builder.ipv6(ip.addr(), ip.netmask());
                        }

                        builder.build_async().map_err(|e| {
                            Error::Endpoint(format!("failed to build tun device: {e}"))
                        })?
                    };

                    info!(
                        "starting tun endpoint, name: {:?}, mtu: {:?}, ip: {:?}",
                        device.name(),
                        device.mtu(),
                        device.addresses()
                    );

                    device
                }

                #[cfg(any(target_os = "android", target_os = "ios"))]
                {
                    return Err(Error::Config(
                        "creating a new tun device is not supported on this platform. A pre-configured 'tun_fd' must be provided.".to_string()
                    ));
                }
            }
        };

        let mut tun_config = TunConfig::default();
        if let Some(value) = &config.fake_dns {
            tun_config.fakedns_cidr = value.parse().map_err(|e| {
                Error::Config(format!("failed to parse fake_dns cidr '{value}': {e}"))
            })?;
        };

        let tun = Tun::new(tun_config.into(), ombrac);
        let shutdown_signal = async {
            let _ = shutdown_rx.recv().await;
        };

        tun.accept_loop(device, shutdown_signal)
            .await
            .map_err(|e| Error::Endpoint(format!("tun device runtime error: {}", e)))
    }
}

async fn quic_client_from_config(config: &ServiceConfig) -> io::Result<QuicClient> {
    let server = &config.server;
    let transport_cfg = &config.transport;

    let server_name = match &transport_cfg.server_name {
        Some(value) => value.clone(),
        None => {
            let pos = server.rfind(':').ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("invalid server address: {}", server),
                )
            })?;
            server[..pos].to_string()
        }
    };

    let addrs: Vec<_> = tokio::net::lookup_host(server).await?.collect();
    let server_addr = addrs.into_iter().next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("failed to resolve server address: '{}'", server),
        )
    })?;

    let mut quic_config = QuicConfig::new(server_addr, server_name);

    quic_config.enable_zero_rtt = transport_cfg.zero_rtt.unwrap_or(false);
    if let Some(protocols) = &transport_cfg.alpn_protocols {
        quic_config.alpn_protocols = protocols.iter().map(|p| p.to_vec()).collect();
    }

    match transport_cfg.tls_mode.unwrap_or(TlsMode::Tls) {
        TlsMode::Tls => {
            if let Some(ca) = &transport_cfg.ca_cert {
                quic_config.root_ca_path = Some(ca.to_path_buf());
            }
        }
        TlsMode::MTls => {
            quic_config.root_ca_path = Some(transport_cfg.ca_cert.clone().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "ca cert is required for mutual tls mode",
                )
            })?);
            let client_cert = transport_cfg.client_cert.clone().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "client cert is required for mutual tls mode",
                )
            })?;
            let client_key = transport_cfg.client_key.clone().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "client key is required for mutual tls mode",
                )
            })?;
            quic_config.client_cert_key_paths = Some((client_cert, client_key));
        }
        TlsMode::Insecure => {
            quic_config.skip_server_verification = true;
        }
    }

    let mut transport_config = QuicTransportConfig::default();
    if let Some(timeout) = transport_cfg.idle_timeout {
        transport_config.max_idle_timeout(Duration::from_millis(timeout))?;
    }
    if let Some(interval) = transport_cfg.keep_alive {
        transport_config.keep_alive_period(Duration::from_millis(interval))?;
    }
    if let Some(max_streams) = transport_cfg.max_streams {
        transport_config.max_open_bidirectional_streams(max_streams)?;
    }
    if let Some(congestion) = transport_cfg.congestion {
        transport_config.congestion(congestion, transport_cfg.cwnd_init)?;
    }
    quic_config.transport_config(transport_config);

    Ok(QuicClient::new(quic_config)?)
}
