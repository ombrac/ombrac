use std::io;
use std::marker::PhantomData;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use ombrac_macros::{error, info, warn};
#[cfg(feature = "transport-quic")]
use ombrac_transport::quic::{
    Connection as QuicConnection, TransportConfig as QuicTransportConfig,
    client::{Client as QuicClient, Config as QuicConfig},
};
use ombrac_transport::{Connection, Initiator};

use crate::client::Client;
use crate::config::ServiceConfig;
#[cfg(feature = "transport-quic")]
use crate::config::TlsMode;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("{0}")]
    Io(#[from] io::Error),

    #[error("Transport layer error: {0}")]
    Transport(String),

    #[error("Endpoint failed: {0}")]
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

pub trait ServiceBuilder {
    type Initiator: Initiator<Connection = Self::Connection>;
    type Connection: Connection;

    fn build(
        config: &Arc<ServiceConfig>,
    ) -> impl Future<Output = Result<Arc<Client<Self::Initiator, Self::Connection>>>> + Send;
}

#[cfg(feature = "transport-quic")]
pub struct QuicServiceBuilder;

#[cfg(feature = "transport-quic")]
impl ServiceBuilder for QuicServiceBuilder {
    type Initiator = QuicClient;
    type Connection = QuicConnection;

    async fn build(
        config: &Arc<ServiceConfig>,
    ) -> Result<Arc<Client<Self::Initiator, Self::Connection>>> {
        let transport = quic_client_from_config(config).await?;
        let secret = *blake3::hash(config.secret.as_bytes()).as_bytes();
        let client = Arc::new(Client::new(transport, secret, None).await?);
        Ok(client)
    }
}

pub struct Service<T, C>
where
    T: Initiator<Connection = C>,
    C: Connection,
{
    handles: Vec<JoinHandle<()>>,
    shutdown_tx: broadcast::Sender<()>,
    _transport: PhantomData<T>,
    _connection: PhantomData<C>,
}

impl<T, C> Service<T, C>
where
    T: Initiator<Connection = C>,
    C: Connection,
{
    pub async fn build<Builder>(config: Arc<ServiceConfig>) -> Result<Self>
    where
        Builder: ServiceBuilder<Initiator = T, Connection = C>,
    {
        let mut handles = Vec::new();
        let client = Builder::build(&config).await?;
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
                "No endpoints were configured or enabled. The service has nothing to do."
                    .to_string(),
            ));
        }

        Ok(Service {
            handles,
            shutdown_tx,
            _transport: PhantomData,
            _connection: PhantomData,
        })
    }

    pub async fn shutdown(self) {
        let _ = self.shutdown_tx.send(());

        for handle in self.handles {
            if let Err(_err) = handle.await {
                error!("A task failed to shut down cleanly: {:?}", _err);
            }
        }
        warn!("Service shutdown complete");
    }

    fn spawn_endpoint(
        _name: &'static str,
        task: impl Future<Output = Result<()>> + Send + 'static,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            if let Err(_e) = task.await {
                error!("The {_name} endpoint shut down due to an error: {_e}");
            }
        })
    }

    #[cfg(feature = "endpoint-http")]
    async fn endpoint_http_accept_loop(
        config: Arc<ServiceConfig>,
        ombrac: Arc<Client<T, C>>,
        mut shutdown_rx: broadcast::Receiver<()>,
    ) -> Result<()> {
        use crate::endpoint::http::Server as HttpServer;
        let bind_addr = require_config!(config.endpoint.http, "endpoint.http")?;
        let socket = tokio::net::TcpListener::bind(bind_addr).await?;

        info!("Starting HTTP/HTTPS endpoint, listening on {bind_addr}");

        HttpServer::new(ombrac)
            .accept_loop(socket, async {
                let _ = shutdown_rx.recv().await;
            })
            .await
            .map_err(|e| Error::Endpoint(format!("HTTP server failed to run: {}", e)))
    }

    #[cfg(feature = "endpoint-socks")]
    async fn endpoint_socks_accept_loop(
        config: Arc<ServiceConfig>,
        ombrac: Arc<Client<T, C>>,
        mut shutdown_rx: broadcast::Receiver<()>,
    ) -> Result<()> {
        use crate::endpoint::socks::CommandHandler;
        use socks_lib::v5::server::auth::NoAuthentication;
        use socks_lib::v5::server::{Config as SocksConfig, Server as SocksServer};

        let bind_addr = require_config!(config.endpoint.socks, "endpoint.socks")?;
        let socket = tokio::net::TcpListener::bind(bind_addr).await?;

        info!("Starting SOCKS5 endpoint, listening on {bind_addr}");

        let socks_config = Arc::new(SocksConfig::new(
            NoAuthentication,
            CommandHandler::new(ombrac),
        ));
        SocksServer::run(socket, socks_config, async {
            let _ = shutdown_rx.recv().await;
        })
        .await
        .map_err(|e| Error::Endpoint(format!("SOCKS server failed to run: {}", e)))
    }

    #[cfg(feature = "endpoint-tun")]
    async fn endpoint_tun_accept_loop(
        config: Arc<ServiceConfig>,
        ombrac: Arc<Client<T, C>>,
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
                                Error::Config(format!("Failed to parse IPv4 CIDR '{ip_str}': {e}"))
                            })?;
                            builder = builder.ipv4(ip.addr(), ip.netmask(), None);
                        }

                        if let Some(ip_str) = &config.tun_ipv6 {
                            let ip = ipnet::Ipv6Net::from_str(ip_str).map_err(|e| {
                                Error::Config(format!("Failed to parse IPv6 CIDR '{ip_str}': {e}"))
                            })?;
                            builder = builder.ipv6(ip.addr(), ip.netmask());
                        }

                        builder.build_async().map_err(|e| {
                            Error::Endpoint(format!("Failed to build TUN device: {e}"))
                        })?
                    };

                    info!(
                        "Starting TUN endpoint, Name: {:?}, MTU: {:?}, IP: {:?}",
                        device.name(),
                        device.mtu(),
                        device.addresses()
                    );

                    device
                }

                #[cfg(any(target_os = "android", target_os = "ios"))]
                {
                    return Err(Error::Config(
                        "Creating a new TUN device is not supported on this platform. A pre-configured 'tun_fd' must be provided.".to_string()
                    ));
                }
            }
        };

        let mut tun_config = TunConfig::default();
        if let Some(value) = &config.fake_dns {
            tun_config.fakedns_cidr = value.parse().map_err(|e| {
                Error::Config(format!("Failed to parse fake_dns CIDR '{value}': {e}"))
            })?;
        };

        let tun = Tun::new(tun_config.into(), ombrac);
        let shutdown_signal = async {
            let _ = shutdown_rx.recv().await;
        };

        tun.accept_loop(device, shutdown_signal)
            .await
            .map_err(|e| Error::Endpoint(format!("TUN device runtime error: {}", e)))
    }
}

#[cfg(feature = "transport-quic")]
async fn quic_client_from_config(config: &ServiceConfig) -> io::Result<QuicClient> {
    let server = &config.server;
    let transport_cfg = &config.transport;

    let server_name = match &transport_cfg.server_name {
        Some(value) => value.clone(),
        None => {
            let pos = server.rfind(':').ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Invalid server address: {}", server),
                )
            })?;
            server[..pos].to_string()
        }
    };

    let addrs: Vec<_> = tokio::net::lookup_host(server).await?.collect();
    let server_addr = addrs.into_iter().next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("Failed to resolve server address: '{}'", server),
        )
    })?;

    let bind_addr = transport_cfg.bind.unwrap_or_else(|| match server_addr {
        SocketAddr::V4(_) => SocketAddr::new(std::net::Ipv4Addr::UNSPECIFIED.into(), 0),
        SocketAddr::V6(_) => SocketAddr::new(std::net::Ipv6Addr::UNSPECIFIED.into(), 0),
    });

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
                    "CA cert is required for mTLS mode",
                )
            })?);
            let client_cert = transport_cfg.client_cert.clone().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Client cert is required for mTLS mode",
                )
            })?;
            let client_key = transport_cfg.client_key.clone().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Client key is required for mTLS mode",
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

    let socket = UdpSocket::bind(bind_addr)?;
    Ok(QuicClient::new(socket, quic_config)?)
}
