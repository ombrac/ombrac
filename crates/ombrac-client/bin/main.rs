use std::error::Error;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use ombrac::Secret;
use ombrac::client::Client;
use ombrac_macros::info;
use ombrac_transport::Initiator;
#[cfg(feature = "transport-quic")]
use ombrac_transport::quic::client::Builder;
use tokio::task::JoinHandle;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Args {
    /// Protocol Secret
    #[clap(
        long,
        short = 'k',
        help_heading = "Service Secret",
        value_name = "STR",
        verbatim_doc_comment
    )]
    secret: String,

    // Endpoint HTTP/HTTPS
    /// The address to bind for the HTTP/HTTPS server
    #[clap(
        long,
        value_name = "ADDR",
        help_heading = "Endpoint HTTP",
        verbatim_doc_comment
    )]
    http: Option<SocketAddr>,

    // Endpoint SOCKS
    /// The address to bind for the SOCKS server
    #[clap(
        long,
        default_value = "127.0.0.1:1080",
        value_name = "ADDR",
        help_heading = "Endpoint SOCKS",
        verbatim_doc_comment
    )]
    socks: SocketAddr,

    // Transport QUIC
    /// The address to bind for QUIC transport
    #[clap(
        long,
        help_heading = "Transport QUIC",
        value_name = "ADDR",
        verbatim_doc_comment
    )]
    bind: Option<SocketAddr>,

    /// Address of the server to connect to
    #[clap(
        long,
        short = 's',
        help_heading = "Transport QUIC",
        value_name = "ADDR",
        verbatim_doc_comment
    )]
    server: String,

    /// Name of the server to connect (derived from `server` if not provided)
    #[clap(
        long,
        help_heading = "Transport QUIC",
        value_name = "STR",
        verbatim_doc_comment
    )]
    server_name: Option<String>,

    /// Path to the TLS certificate file
    #[clap(
        long,
        help_heading = "Transport QUIC",
        value_name = "FILE",
        verbatim_doc_comment
    )]
    tls_cert: Option<PathBuf>,

    /// Skip TLS certificate verification (insecure, for testing only)
    #[clap(long, help_heading = "Transport QUIC", action, verbatim_doc_comment)]
    insecure: bool,

    /// Enable 0-RTT for faster connection establishment (may reduce security)
    #[clap(long, help_heading = "Transport QUIC", action, verbatim_doc_comment)]
    zero_rtt: bool,

    /// Disable connection multiplexing (each stream uses a separate QUIC connection)
    /// This may be useful in special network environments where multiplexing causes issues
    #[clap(long, help_heading = "Transport QUIC", action, verbatim_doc_comment)]
    no_multiplex: bool,

    /// Initial congestion window size in bytes
    // #[clap(
    //     long,
    //     help_heading = "Transport QUIC",
    //     value_name = "NUM",
    //     verbatim_doc_comment
    // )]
    // cwnd_init: Option<u64>,

    /// Maximum idle time (in milliseconds) before closing the connection
    /// 30 second default recommended by RFC 9308
    #[clap(
        long,
        help_heading = "Transport QUIC",
        value_name = "TIME",
        default_value = "30000",
        verbatim_doc_comment
    )]
    idle_timeout: Option<u64>,

    /// Keep-alive interval (in milliseconds)
    #[clap(
        long,
        help_heading = "Transport QUIC",
        value_name = "TIME",
        default_value = "8000",
        verbatim_doc_comment
    )]
    keep_alive: Option<u64>,

    /// Maximum number of bidirectional streams that can be open simultaneously
    #[clap(
        long,
        help_heading = "Transport QUIC",
        value_name = "NUM",
        default_value = "100",
        verbatim_doc_comment
    )]
    max_streams: Option<u64>,

    /// Try to resolve domain name to IPv6 addresses first
    #[clap(
        short = '6',
        help_heading = "Transport QUIC",
        action,
        verbatim_doc_comment
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
async fn main() -> Result<(), Box<dyn Error>> {
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
        let secret = blake3::hash(args.secret.as_bytes());
        let ombrac_client = Client::new(
            quic_from_args(&args)
                .await?
                .build()
                .await
                .expect("QUIC Client failed to build"),
        );

        let client = Arc::new(ombrac_client);
        let secret = *secret.as_bytes();

        #[cfg(feature = "endpoint-http")]
        if let Some(address) = args.http {
            let client = client.clone();
            run_http_server(client, secret, address, async {
                tokio::signal::ctrl_c()
                    .await
                    .expect("Failed to listen for event");
            })
            .await?;
        }

        #[cfg(feature = "endpoint-socks")]
        run_socks_server(client, secret, args.socks, async {
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to listen for event");
        })
        .await?
        .await?;
    }

    Ok(())
}

#[cfg(feature = "transport-quic")]
async fn quic_from_args(args: &Args) -> Result<Builder, Box<dyn Error>> {
    use std::time::Duration;
    use tokio::net::lookup_host;

    let name = match &args.server_name {
        Some(value) => value.to_string(),
        None => {
            let pos = args
                .server
                .rfind(':')
                .ok_or(format!("Invalid server address {}", args.server))?;

            args.server[..pos].to_string()
        }
    };

    let mut addrs: Vec<_> = lookup_host(&args.server).await?.collect();

    if args.prefer_ipv6 {
        addrs.sort_by(|a, b| match (a.is_ipv6(), b.is_ipv6()) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => std::cmp::Ordering::Equal,
        });
    }

    let addr = addrs.into_iter().next().ok_or(format!(
        "Failed to resolve server address '{}'",
        args.server
    ))?;

    let mut builder = Builder::new(addr, name);

    if let Some(value) = args.bind {
        builder.with_bind(value);
    }

    if let Some(value) = &args.server_name {
        builder.with_server_name(value.to_string());
    }

    if let Some(value) = &args.tls_cert {
        builder.with_tls(value.clone());
    }

    // if let Some(value) = args.cwnd_init {
    //     builder.with_congestion_initial_window(value);
    // }

    if let Some(value) = args.idle_timeout {
        builder.with_max_idle_timeout(Duration::from_millis(value))?;
    }

    if let Some(value) = args.keep_alive {
        builder.with_max_keep_alive_period(Duration::from_millis(value));
    }

    if let Some(value) = args.max_streams {
        builder.with_max_open_bidirectional_streams(value)?;
    }

    builder.with_tls_skip(args.insecure);
    builder.with_enable_zero_rtt(args.zero_rtt);
    builder.with_enable_connection_multiplexing(!args.no_multiplex);

    Ok(builder)
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
        info!("HTTP Server Listening on {}", address);

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
