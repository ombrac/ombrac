use std::error::Error;
use std::net::SocketAddr;
use std::time::Duration;

use clap::Parser;
use ombrac_client::endpoint::socks::{Config as SocksConfig, SocksServer};
use ombrac_client::transport::quic::{Config as QuicConfig, NoiseQuic};
use ombrac_client::Client;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Args {
    // Endpoint SOCKS
    /// Listening address for the SOCKS server.
    #[clap(
        long,
        default_value = "127.0.0.1:1080",
        value_name = "ADDR",
        help_heading = "Endpoint SOCKS"
    )]
    socks: String,

    // Transport QUIC
    /// Bind local address
    #[clap(long, help_heading = "Transport QUIC", value_name = "ADDR")]
    bind: Option<SocketAddr>,

    /// Name of the server to connect to.
    #[clap(long, help_heading = "Transport QUIC", value_name = "STR")]
    server_name: Option<String>,

    /// Address of the server to connect to.
    #[clap(long, help_heading = "Transport QUIC", value_name = "ADDR")]
    server_address: String,

    /// Path to the TLS certificate file for secure connections.
    #[clap(long, help_heading = "Transport QUIC", value_name = "FILE")]
    tls_cert: Option<String>,

    /// Initial congestion window in bytes
    #[clap(
        long,
        help_heading = "Transport QUIC",
        value_name = "NUM",
        default_value = "32"
    )]
    initial_congestion_window: u32,

    /// Connection multiplexing
    #[clap(
        long,
        help_heading = "Transport QUIC",
        value_name = "NUM",
        default_value = "0"
    )]
    max_multiplex: u64,

    /// Handshake timeout in millisecond.
    #[clap(
        long,
        help_heading = "Transport QUIC",
        value_name = "TIME",
        default_value = "3000"
    )]
    max_handshake_duration: u64,

    /// Connection idle timeout in millisecond.
    #[clap(
        long,
        help_heading = "Transport QUIC",
        value_name = "TIME",
        default_value = "0"
    )]
    max_idle_timeout: u64,

    /// Connection keep alive period in millisecond.
    #[clap(
        long,
        help_heading = "Transport QUIC",
        value_name = "TIME",
        default_value = "8000"
    )]
    max_keep_alive_period: u64,

    /// Connection max open bidirectional streams.
    #[clap(
        long,
        help_heading = "Transport QUIC",
        value_name = "NUM",
        default_value = "100"
    )]
    max_open_bidirectional_streams: u64,

    /// Bidirectional stream local data window.
    #[clap(
        long,
        help_heading = "Transport QUIC",
        value_name = "NUM",
        default_value = "3750000"
    )]
    bidirectional_local_data_window: u64,

    /// Bidirectional stream remote data window.
    #[clap(
        long,
        help_heading = "Transport QUIC",
        value_name = "NUM",
        default_value = "3750000"
    )]
    bidirectional_remote_data_window: u64,

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
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    #[cfg(feature = "tracing")]
    tracing_subscriber::fmt()
        .with_thread_ids(true)
        .with_max_level(args.tracing_level)
        .init();

    let endpoint = SocksServer::with(socks_config_from_args(&args)?).await?;
    let transport = NoiseQuic::with(quic_config_from_args(&args)?).await?;

    Client::with(endpoint, transport).start().await;

    Ok(())
}

fn socks_config_from_args(args: &Args) -> Result<SocksConfig, Box<dyn Error>> {
    let listen: SocketAddr = args.socks.parse()?;

    Ok(SocksConfig::new(listen))
}

fn quic_config_from_args(args: &Args) -> Result<QuicConfig, Box<dyn Error>> {
    let mut config = QuicConfig::with_address(args.server_address.to_string())?;

    if let Some(bind) = args.bind {
        config = config.with_bind(bind);
    }

    if let Some(name) = args.server_name.clone() {
        config = config.with_server_name(name);
    }

    if let Some(tls_cert) = args.tls_cert.clone() {
        config = config.with_tls_cert(tls_cert);
    }

    config = config
        .with_initial_congestion_window(args.initial_congestion_window)
        .with_max_handshake_duration(Duration::from_millis(args.max_handshake_duration))
        .with_max_multiplex(args.max_multiplex)
        .with_max_idle_timeout(Duration::from_millis(args.max_idle_timeout))
        .with_max_keep_alive_period(Duration::from_millis(args.max_keep_alive_period))
        .with_max_open_bidirectional_streams(args.max_open_bidirectional_streams)
        .with_bidirectional_local_data_window(args.bidirectional_local_data_window)
        .with_bidirectional_remote_data_window(args.bidirectional_remote_data_window);

    Ok(config)
}
