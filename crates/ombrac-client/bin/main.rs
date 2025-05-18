use std::error::Error;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use ombrac_client::Client;
use ombrac_client::endpoint::http::Server as HttpServer;
use ombrac_client::endpoint::socks::Server as SocksServer;
use ombrac_client::transport::quic::Builder;

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
    #[clap(
        long,
        help_heading = "Transport QUIC",
        value_name = "NUM",
        verbatim_doc_comment
    )]
    cwnd_init: Option<u64>,

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

    let secret = blake3::hash(args.secret.as_bytes());
    let ombrac_client = Client::new(
        *secret.as_bytes(),
        quic_from_args(&args)
            .await?
            .build()
            .await
            .expect("QUIC Client failed to build"),
    );

    let client = Arc::new(ombrac_client);
    tokio::time::sleep(Duration::from_millis(100)).await;

    if let Some(addr) = args.http {
        let client = client.clone();
        tokio::spawn(async move {
            HttpServer::bind(addr, client)
                .await
                .expect("HTTP server failed to bind")
                .listen()
                .await
                .expect("HTTP server failed to start");
        });
    }

    SocksServer::bind(args.socks, client)
        .await
        .expect("SOCKS server failed to bind")
        .listen()
        .await?;

    Ok(())
}

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
        builder = builder.with_bind(value);
    }

    if let Some(value) = &args.server_name {
        builder = builder.with_server_name(value.to_string());
    }

    if let Some(value) = &args.tls_cert {
        builder = builder.with_tls_cert(value.clone());
    }

    if let Some(value) = args.cwnd_init {
        builder = builder.with_congestion_initial_window(value);
    }

    if let Some(value) = args.idle_timeout {
        builder = builder.with_max_idle_timeout(Duration::from_millis(value));
    }

    if let Some(value) = args.keep_alive {
        builder = builder.with_max_keep_alive_period(Duration::from_millis(value));
    }

    if let Some(value) = args.max_streams {
        builder = builder.with_max_open_bidirectional_streams(value);
    }

    builder = builder.with_tls_skip(args.insecure);
    builder = builder.with_enable_zero_rtt(args.zero_rtt);
    builder = builder.with_enable_connection_multiplexing(!args.no_multiplex);

    Ok(builder)
}
