use std::error::Error;
use std::net::SocketAddr;

use clap::Parser;
use ombrac_client::endpoint::socks::{Config as SocksServerConfig, Server as SocksServer};
use ombrac_client::transport::quic::Config as QuicConfig;
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
    bind: Option<String>,

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
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    #[cfg(feature = "tracing")]
    tracing_subscriber::fmt()
        .with_thread_ids(true)
        .with_max_level(args.tracing_level)
        .init();

    let transport = Client::new(quic_config_from_args(&args)?).await?;

    let endpoint = SocksServer::new(socks_config_from_args(&args)?, transport);

    endpoint.listen().await?;

    Ok(())
}

fn socks_config_from_args(args: &Args) -> Result<SocksServerConfig, Box<dyn Error>> {
    let listen: SocketAddr = args.socks.parse()?;

    Ok(SocksServerConfig::new(listen.to_string()))
}

fn quic_config_from_args(args: &Args) -> Result<QuicConfig, Box<dyn std::error::Error>> {
    use std::time::Duration;

    let mut config = QuicConfig::new(args.server_address.clone());

    if let Some(value) = &args.bind {
        config = config.with_bind(value.to_string());
    }

    if let Some(value) = &args.server_name {
        config = config.with_server_name(value.to_string());
    }

    if let Some(value) = &args.tls_cert {
        config = config.with_tls_cert(value.to_string());
    }

    if let Some(value) = args.initial_congestion_window {
        config = config.with_initial_congestion_window(value);
    }

    if let Some(value) = args.max_handshake_duration {
        config = config.with_max_handshake_duration(Duration::from_millis(value));
    }

    if let Some(value) = args.max_idle_timeout {
        config = config.with_max_idle_timeout(Duration::from_millis(value));
    }

    if let Some(value) = args.max_keep_alive_period {
        config = config.with_max_keep_alive_period(Duration::from_millis(value));
    }

    if let Some(value) = args.max_open_bidirectional_streams {
        config = config.with_max_open_bidirectional_streams(value);
    }

    if let Some(value) = args.bidirectional_local_data_window {
        config = config.with_bidirectional_local_data_window(value);
    }

    if let Some(value) = args.bidirectional_remote_data_window {
        config = config.with_bidirectional_remote_data_window(value);
    }

    Ok(config)
}
