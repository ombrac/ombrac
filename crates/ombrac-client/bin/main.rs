use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::builder::Styles;
use clap::builder::styling::{AnsiColor, Style};
use clap::{Parser, ValueEnum};
use ombrac::Secret;
use ombrac::client::Client;
use ombrac_macros::{error, info};
use ombrac_transport::Initiator;
#[cfg(feature = "transport-quic")]
use ombrac_transport::quic::{
    Congestion, TransportConfig,
    client::{Client as QuicClient, Config},
};
use tokio::task::JoinHandle;

fn styles() -> Styles {
    Styles::styled()
        .header(Style::new().bold().fg_color(Some(AnsiColor::Green.into())))
        .usage(Style::new().bold().fg_color(Some(AnsiColor::Green.into())))
        .literal(Style::new().bold().fg_color(Some(AnsiColor::Cyan.into())))
        .placeholder(Style::new().fg_color(Some(AnsiColor::Cyan.into())))
        .valid(Style::new().bold().fg_color(Some(AnsiColor::Cyan.into())))
        .invalid(Style::new().bold().fg_color(Some(AnsiColor::Yellow.into())))
        .error(Style::new().bold().fg_color(Some(AnsiColor::Red.into())))
}

#[cfg(feature = "transport-quic")]
#[derive(ValueEnum, Clone, Debug, Copy)]
enum TlsMode {
    Tls,
    MTls,
    Insecure,
}

#[derive(Debug, Parser)]
#[command(version, about, long_about = None, styles = styles() )]
struct Args {
    /// Protocol Secret
    #[clap(
        long,
        short = 'k',
        help_heading = "Required",
        value_name = "STR",
        verbatim_doc_comment
    )]
    secret: String,

    /// Address of the server to connect to
    #[clap(
        long,
        short = 's',
        help_heading = "Required",
        value_name = "ADDR",
        verbatim_doc_comment
    )]
    server: String,

    // Endpoint HTTP/HTTPS
    /// The address to bind for the HTTP/HTTPS server
    #[clap(
        long,
        value_name = "ADDR",
        help_heading = "Endpoint",
        verbatim_doc_comment
    )]
    http: Option<SocketAddr>,

    // Endpoint SOCKS
    /// The address to bind for the SOCKS server
    #[clap(
        long,
        default_value = "127.0.0.1:1080",
        value_name = "ADDR",
        help_heading = "Endpoint",
        verbatim_doc_comment
    )]
    socks: SocketAddr,

    // Transport QUIC
    /// The address to bind for transport
    #[clap(
        long,
        help_heading = "Transport",
        value_name = "ADDR",
        verbatim_doc_comment
    )]
    bind: Option<SocketAddr>,

    /// Name of the server to connect (derived from `server` if not provided)
    #[clap(
        long,
        help_heading = "Transport",
        value_name = "STR",
        verbatim_doc_comment
    )]
    server_name: Option<String>,

    /// Set the TLS mode for the connection
    /// tls: Standard TLS with server certificate verification
    /// m-tls: Mutual TLS with client and server certificate verification
    /// insecure: Skip server certificate verification (for testing only)
    #[cfg(feature = "transport-quic")]
    #[clap(
        long,
        value_enum,
        default_value_t = TlsMode::Tls,
        help_heading = "Transport",
        verbatim_doc_comment
    )]
    tls_mode: TlsMode,

    /// Path to the Certificate Authority (CA) certificate file
    /// in 'TLS' mode, if not provided, the system's default root certificates are used
    #[cfg(feature = "transport-quic")]
    #[clap(
        long,
        help_heading = "Transport",
        value_name = "FILE",
        verbatim_doc_comment
    )]
    ca_cert: Option<PathBuf>,

    /// Path to the client's TLS certificate for mTLS
    #[cfg(feature = "transport-quic")]
    #[clap(
        long,
        help_heading = "Transport",
        value_name = "FILE",
        verbatim_doc_comment
    )]
    client_cert: Option<PathBuf>,

    /// Path to the client's TLS private key for mTLS
    #[cfg(feature = "transport-quic")]
    #[clap(
        long,
        help_heading = "Transport",
        value_name = "FILE",
        verbatim_doc_comment
    )]
    client_key: Option<PathBuf>,

    /// Enable 0-RTT for faster connection establishment (may reduce security)
    #[cfg(feature = "transport-quic")]
    #[clap(long, help_heading = "Transport", action, verbatim_doc_comment)]
    zero_rtt: bool,

    /// Application-Layer protocol negotiation (ALPN) protocols
    /// e.g. "h3,h3-29"
    #[clap(
        long,
        help_heading = "Transport",
        value_name = "PROTOCOLS",
        default_value = "h3",
        value_delimiter = ',',
        verbatim_doc_comment
    )]
    #[cfg(feature = "transport-quic")]
    alpn_protocols: Vec<Vec<u8>>,

    /// Congestion control algorithm to use (e.g. bbr, cubic, newreno)
    #[cfg(feature = "transport-quic")]
    #[clap(
        long,
        help_heading = "Transport",
        value_name = "ALGORITHM",
        default_value = "bbr",
        verbatim_doc_comment
    )]
    congestion: Congestion,

    /// Initial congestion window size in bytes
    #[cfg(feature = "transport-quic")]
    #[clap(
        long,
        help_heading = "Transport",
        value_name = "NUM",
        verbatim_doc_comment
    )]
    cwnd_init: Option<u64>,

    /// Maximum idle time (in milliseconds) before closing the connection
    /// 30 second default recommended by RFC 9308
    #[cfg(feature = "transport-quic")]
    #[clap(
        long,
        help_heading = "Transport",
        value_name = "TIME",
        default_value = "30000",
        verbatim_doc_comment
    )]
    idle_timeout: u64,

    /// Keep-alive interval (in milliseconds)
    #[cfg(feature = "transport-quic")]
    #[clap(
        long,
        help_heading = "Transport",
        value_name = "TIME",
        default_value = "8000",
        verbatim_doc_comment
    )]
    keep_alive: u64,

    /// Maximum number of bidirectional streams that can be open simultaneously
    #[cfg(feature = "transport-quic")]
    #[clap(
        long,
        help_heading = "Transport",
        value_name = "NUM",
        default_value = "100",
        verbatim_doc_comment
    )]
    max_streams: u64,

    /// Try to resolve domain name to IPv4 addresses first
    #[cfg(feature = "transport-quic")]
    #[clap(
        long,
        short = '4',
        help_heading = "Transport",
        action,
        verbatim_doc_comment,
        conflicts_with = "prefer_ipv6"
    )]
    prefer_ipv4: bool,

    /// Try to resolve domain name to IPv6 addresses first
    #[cfg(feature = "transport-quic")]
    #[clap(
        long,
        short = '6',
        help_heading = "Transport",
        action,
        verbatim_doc_comment,
        conflicts_with = "prefer_ipv4"
    )]
    prefer_ipv6: bool,

    /// Logging level (e.g., INFO, WARN, ERROR)
    #[cfg(feature = "tracing")]
    #[clap(
        long,
        default_value = "WARN",
        value_name = "LEVEL",
        help_heading = "Logging",
        verbatim_doc_comment
    )]
    log_level: tracing::Level,

    /// Path to the log directory
    #[cfg(feature = "tracing")]
    #[clap(
        long,
        value_name = "PATH",
        help_heading = "Logging",
        verbatim_doc_comment
    )]
    log_dir: Option<PathBuf>,

    /// Prefix for log file names (only used when log-dir is specified)
    #[cfg(feature = "tracing")]
    #[clap(
        long,
        default_value = "log",
        value_name = "STR",
        help_heading = "Logging",
        verbatim_doc_comment
    )]
    log_prefix: PathBuf,
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let args = Args::parse();

    #[cfg(feature = "tracing")]
    {
        let subscriber = tracing_subscriber::fmt()
            .with_thread_ids(true)
            .with_max_level(args.log_level);

        let (non_blocking, guard) = if let Some(path) = &args.log_dir {
            let file_appender = tracing_appender::rolling::daily(path, &args.log_prefix);
            tracing_appender::non_blocking(file_appender)
        } else {
            tracing_appender::non_blocking(std::io::stdout())
        };

        std::mem::forget(guard);
        subscriber.with_writer(non_blocking).init()
    }

    #[cfg(feature = "transport-quic")]
    {
        let secret = *blake3::hash(args.secret.as_bytes()).as_bytes();
        let transport = quic_client_from_args(&args).await?;
        let client = Arc::new(Client::new(transport));

        let mut handles = Vec::new();
        let (shutdown_tx, _) = tokio::sync::broadcast::channel(1);

        #[cfg(feature = "endpoint-http")]
        {
            if let Some(address) = args.http {
                let client = client.clone();
                let mut http_shutdown_rx = shutdown_tx.subscribe();

                let http_handle = run_http_server(client, secret, address, async move {
                    let _ = http_shutdown_rx.recv().await;
                })
                .await?;
                handles.push(http_handle);
            }
        }

        #[cfg(feature = "endpoint-socks")]
        {
            let mut socks_shutdown_rx = shutdown_tx.subscribe();
            let socks_handle = run_socks_server(client, secret, args.socks, async move {
                let _ = socks_shutdown_rx.recv().await;
            })
            .await?;
            handles.push(socks_handle);
        }

        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                let _ = shutdown_tx.send(());
            },
        }

        for handle in handles {
            if let Err(e) = handle.await {
                error!("Error waiting for task to shut down: {:?}", e);
            }
        }
    }

    Ok(())
}

#[cfg(feature = "endpoint-http")]
async fn run_http_server<I: Initiator>(
    ombrac: Arc<Client<I>>,
    secret: Secret,
    address: SocketAddr,
    shutdown_signal: impl Future<Output = ()> + Send + 'static,
) -> io::Result<JoinHandle<()>> {
    use ombrac_client::endpoint::http::Server as HttpServer;
    use tokio::net::TcpListener;

    let listener = TcpListener::bind(address).await?;

    let handle = tokio::spawn(async move {
        info!("HTTP/HTTPS Server Listening on {}", address);

        HttpServer::run(listener, secret, ombrac, shutdown_signal)
            .await
            .expect("Failed to run HTTP server");
    });

    Ok(handle)
}

#[cfg(feature = "endpoint-socks")]
async fn run_socks_server<I: Initiator>(
    ombrac: Arc<Client<I>>,
    secret: Secret,
    address: SocketAddr,
    shutdown_signal: impl Future<Output = ()> + Send + 'static,
) -> io::Result<JoinHandle<()>> {
    use ombrac_client::endpoint::socks::CommandHandler;
    use socks_lib::net::TcpListener;
    use socks_lib::v5::server::auth::NoAuthentication;
    use socks_lib::v5::server::{Config, Server as SocksServer};

    let listener = TcpListener::bind(address).await?;

    let handle = tokio::spawn(async move {
        info!("SOCKS Server Listening on {address}");

        let config = Config::new(NoAuthentication, CommandHandler::new(ombrac, secret));

        SocksServer::run(listener, config.into(), shutdown_signal)
            .await
            .expect("Failed to run SOCKS server");
    });

    Ok(handle)
}

#[cfg(feature = "transport-quic")]
async fn quic_client_from_args(args: &Args) -> io::Result<QuicClient> {
    let server_name = match &args.server_name {
        Some(value) => value.clone(),
        None => {
            let pos = args.server.rfind(':').ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Invalid server address {}", args.server),
                )
            })?;
            args.server[..pos].to_string()
        }
    };

    let mut addrs: Vec<_> = tokio::net::lookup_host(&args.server).await?.collect();

    if args.prefer_ipv6 {
        addrs.sort_by(|a, b| match (a.is_ipv6(), b.is_ipv6()) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => std::cmp::Ordering::Equal,
        });
    } else if args.prefer_ipv4 {
        addrs.sort_by(|a, b| match (a.is_ipv4(), b.is_ipv4()) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => std::cmp::Ordering::Equal,
        });
    }

    let server_addr = addrs.into_iter().next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("Failed to resolve server address '{}'", args.server),
        )
    })?;

    let bind_addr = match args.bind {
        Some(addr) => addr,
        None => match server_addr {
            SocketAddr::V4(_) => SocketAddr::new(std::net::Ipv4Addr::UNSPECIFIED.into(), 0),
            SocketAddr::V6(_) => SocketAddr::new(std::net::Ipv6Addr::UNSPECIFIED.into(), 0),
        },
    };

    let mut config = Config::new(server_addr, server_name);

    config.enable_zero_rtt = args.zero_rtt;
    config.alpn_protocols = args.alpn_protocols.clone();
    match args.tls_mode {
        TlsMode::Tls => {
            if let Some(ca) = &args.ca_cert {
                config.root_ca_path = Some(ca.clone());
            }
        }
        TlsMode::MTls => {
            config.root_ca_path = Some(args.ca_cert.clone().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "--ca-cert is required for mTLS mode",
                )
            })?);
            let client_cert = args.client_cert.clone().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "--client-cert is required for mTLS mode",
                )
            })?;
            let client_key = args.client_key.clone().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "--client-key is required for mTLS mode",
                )
            })?;
            config.client_cert_key_paths = Some((client_cert, client_key));
        }
        TlsMode::Insecure => {
            config.skip_server_verification = true;
        }
    }

    let mut transport_config = TransportConfig::default();
    transport_config.max_idle_timeout(Duration::from_millis(args.idle_timeout))?;
    transport_config.keep_alive_period(Duration::from_millis(args.keep_alive))?;
    transport_config.max_open_bidirectional_streams(args.max_streams)?;
    transport_config.congestion(args.congestion, args.cwnd_init)?;
    config.transport_config(transport_config);

    let socket = UdpSocket::bind(bind_addr)?;
    Ok(QuicClient::new(config, socket).await?)
}
