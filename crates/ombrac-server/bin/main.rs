use std::error::Error;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use ombrac_server::Server;
use ombrac_transport::quic::server::Builder;

#[derive(Parser, Debug)]
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

    // Transport QUIC
    /// The address to bind for QUIC transport
    #[clap(
        long,
        short = 'l',
        help_heading = "Transport QUIC",
        value_name = "ADDR",
        verbatim_doc_comment
    )]
    listen: SocketAddr,

    /// Path to the TLS certificate file
    #[clap(
        long,
        help_heading = "Transport QUIC",
        value_name = "FILE",
        verbatim_doc_comment
    )]
    tls_cert: Option<PathBuf>,

    /// Path to the TLS private key file
    #[clap(
        long,
        help_heading = "Transport QUIC",
        value_name = "FILE",
        verbatim_doc_comment
    )]
    tls_key: Option<PathBuf>,

    /// When enabled, the server will generate a self-signed TLS certificate
    /// and use it for the QUIC connection. This mode is useful for testing
    /// but should not be used in production
    #[clap(long, help_heading = "Transport QUIC", action, verbatim_doc_comment)]
    insecure: bool,

    /// Enable 0-RTT for faster connection establishment (may reduce security)
    #[clap(long, help_heading = "Transport QUIC", action, verbatim_doc_comment)]
    zero_rtt: bool,

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
    let transport = quic_config_from_args(&args)
        .build()
        .await
        .expect("QUIC Server failed to build");

    let ombrac_server = Server::new(*secret.as_bytes(), transport);

    #[cfg(feature = "tracing")]
    tracing::info!("Server listening on {}", args.listen);

    ombrac_server
        .listen()
        .await
        .expect("Server failed to listen");

    Ok(())
}

fn quic_config_from_args(args: &Args) -> Builder {
    let mut builder = Builder::new(args.listen);

    if let Some(value) = &args.tls_cert {
        builder = builder.with_tls_cert(value.clone())
    }

    if let Some(value) = &args.tls_key {
        builder = builder.with_tls_key(value.clone())
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

    builder
}
