use std::error::Error;
use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use ombrac_server::transport::quic::{Builder, Quic};
use ombrac_server::Server;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    // Transport QUIC
    /// Transport server listening address
    #[clap(long, help_heading = "Transport QUIC", value_name = "ADDR")]
    listen: String,

    /// Path to the TLS certificate file for secure connections
    #[clap(long, help_heading = "Transport QUIC", value_name = "FILE")]
    tls_cert: PathBuf,

    /// Path to the TLS private key file for secure connections
    #[clap(long, help_heading = "Transport QUIC", value_name = "FILE")]
    tls_key: PathBuf,

    /// Initial congestion window in bytes
    #[clap(long, help_heading = "Transport QUIC", value_name = "NUM")]
    initial_congestion_window: Option<u32>,

    /// Handshake timeout in millisecond
    #[clap(long, help_heading = "Transport QUIC", value_name = "TIME")]
    max_handshake_duration: Option<u64>,

    /// Connection idle timeout in millisecond
    #[clap(long, help_heading = "Transport QUIC", value_name = "TIME")]
    max_idle_timeout: Option<u64>,

    /// Connection keep alive period in millisecond
    #[clap(long, help_heading = "Transport QUIC", value_name = "TIME")]
    max_keep_alive_period: Option<u64>,

    /// Connection max open bidirectional streams
    #[clap(long, help_heading = "Transport QUIC", value_name = "NUM")]
    max_open_bidirectional_streams: Option<u64>,

    /// Bidirectional stream local data window
    #[clap(long, help_heading = "Transport QUIC", value_name = "NUM")]
    bidirectional_local_data_window: Option<u64>,

    /// Bidirectional stream remote data window
    #[clap(long, help_heading = "Transport QUIC", value_name = "NUM")]
    bidirectional_remote_data_window: Option<u64>,

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

    let mut server = Server::new(quic_config_from_args(&args).await?);

    tracing::info!("Server listening on {}", args.listen);

    server.listen().await?;

    Ok(())
}

async fn quic_config_from_args(args: &Args) -> Result<Quic, Box<dyn Error>> {
    let mut builder = Builder::new(
        args.listen.to_string(),
        args.tls_cert.clone(),
        args.tls_key.clone(),
    );

    if let Some(value) = args.initial_congestion_window {
        builder = builder.with_initial_congestion_window(value);
    }

    if let Some(value) = args.max_handshake_duration {
        builder = builder.with_max_handshake_duration(Duration::from_millis(value));
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

    if let Some(value) = args.bidirectional_local_data_window {
        builder = builder.with_bidirectional_local_data_window(value);
    }

    if let Some(value) = args.bidirectional_remote_data_window {
        builder = builder.with_bidirectional_remote_data_window(value);
    }

    Ok(builder.build().await?)
}
