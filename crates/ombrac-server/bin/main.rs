use std::error::Error;
use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use ombrac_server::Server;
use ombrac_transport::quic::server::Builder;
use ombrac_transport::quic::Connection;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Protocol Secret
    #[clap(long, short = 'k', help_heading = "Service Secret", value_name = "STR")]
    secret: String,

    // Transport QUIC
    /// Transport server listening address
    #[clap(long, short = 'l', help_heading = "Transport QUIC", value_name = "ADDR")]
    listen: String,

    /// Path to the TLS certificate file for secure connections
    #[clap(long, help_heading = "Transport QUIC", value_name = "FILE")]
    tls_cert: PathBuf,

    /// Path to the TLS private key file for secure connections
    #[clap(long, help_heading = "Transport QUIC", value_name = "FILE")]
    tls_key: PathBuf,

    /// Whether to enable 0-RTT or 0.5-RTT connections at the cost of weakened security
    #[clap(long, help_heading = "Transport QUIC", value_name = "BOOL")]
    enable_zero_rtt: Option<bool>,

    /// Initial congestion window in bytes
    #[clap(long, help_heading = "Transport QUIC", value_name = "NUM")]
    congestion_initial_window: Option<u64>,

    /// Connection idle timeout in millisecond
    #[clap(long, help_heading = "Transport QUIC", value_name = "TIME")]
    max_idle_timeout: Option<u64>,

    /// Connection keep alive period in millisecond
    #[clap(long, help_heading = "Transport QUIC", value_name = "TIME")]
    max_keep_alive_period: Option<u64>,

    /// Connection max open bidirectional streams
    #[clap(long, help_heading = "Transport QUIC", value_name = "NUM")]
    max_open_bidirectional_streams: Option<u64>,

    /// Logging level e.g., INFO, WARN, ERROR
    #[cfg(feature = "tracing")]
    #[clap(
        long,
        default_value = "WARN",
        value_name = "TRACE",
        help_heading = "Logging"
    )]
    tracing_level: tracing::Level,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    #[cfg(feature = "tracing")]
    tracing_subscriber::fmt()
        .with_thread_ids(true)
        .with_max_level(args.tracing_level)
        .init();

    let secret = blake3::hash(args.secret.as_bytes());
    let ombrac_server = Server::new(*secret.as_bytes(), quic_config_from_args(&args).await?);

    #[cfg(feature = "tracing")]
    tracing::info!("Server listening on {}", args.listen);

    ombrac_server.listen().await?;

    Ok(())
}

async fn quic_config_from_args(args: &Args) -> Result<Connection, Box<dyn Error>> {
    let mut builder = Builder::new(
        args.listen.to_string(),
        args.tls_cert.clone(),
        args.tls_key.clone(),
    );

    if let Some(value) = args.enable_zero_rtt {
        builder = builder.with_enable_zero_rtt(value);
    }

    if let Some(value) = args.congestion_initial_window {
        builder = builder.with_congestion_initial_window(value);
    }

    if let Some(value) = args.max_idle_timeout {
        builder = builder.with_max_idle_timeout(Duration::from_millis(value));
    }

    if let Some(value) = args.max_keep_alive_period {
        builder = builder.with_max_keep_alive_period(Duration::from_millis(value));
    }

    if let Some(value) = args.max_open_bidirectional_streams {
        builder = builder.with_max_open_bidirectional_streams(value);
    }

    Ok(builder.build().await?)
}
