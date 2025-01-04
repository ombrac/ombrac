use std::error::Error;
use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;
use ombrac_client::endpoint::socks::Server as SocksServer;
use ombrac_client::Client;
use ombrac_transport::quic::client::Builder;
use ombrac_transport::quic::Connection;

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
    socks: SocketAddr,

    // Transport QUIC
    /// Bind address
    #[clap(long, help_heading = "Transport QUIC", value_name = "ADDR")]
    bind: Option<String>,

    /// Name of the server to connect
    #[clap(long, help_heading = "Transport QUIC", value_name = "STR")]
    server_name: Option<String>,

    /// Address of the server to connect
    #[clap(long, help_heading = "Transport QUIC", value_name = "ADDR")]
    server_address: String,

    /// Path to the TLS certificate file for secure connections
    #[clap(long, help_heading = "Transport QUIC", value_name = "FILE")]
    tls_cert: Option<PathBuf>,

    /// Whether to enable 0-RTT or 0.5-RTT connections at the cost of weakened security
    #[clap(long, help_heading = "Transport QUIC", value_name = "BOOL")]
    enable_zero_rtt: Option<bool>,

    /// Whether to enable connection multiplexing
    #[clap(long, help_heading = "Transport QUIC", value_name = "BOOL")]
    enable_connection_multiplexing: Option<bool>,

    /// Initial congestion window in bytes
    #[clap(long, help_heading = "Transport QUIC", value_name = "NUM")]
    congestion_initial_window: Option<u64>,

    /// Connection idle timeout in millisecond
    #[clap(long, help_heading = "Transport QUIC", value_name = "TIME")]
    max_idle_timeout: Option<u64>,

    /// Connection keep alive period in millisecond
    #[clap(long, help_heading = "Transport QUIC", value_name = "TIME", default_value = "8000")]
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

    let ombrac_client = Client::new(quic_from_args(&args).await?);

    SocksServer::listen(args.socks, ombrac_client).await?;

    Ok(())
}

async fn quic_from_args(args: &Args) -> Result<Connection, Box<dyn Error>> {
    use std::time::Duration;

    let mut builder = Builder::new(args.server_address.clone());

    if let Some(value) = &args.bind {
        builder = builder.with_bind(value.to_string());
    }

    if let Some(value) = &args.server_name {
        builder = builder.with_server_name(value.to_string());
    }

    if let Some(value) = &args.tls_cert {
        builder = builder.with_tls_cert(value.clone());
    }

    if let Some(value) = &args.enable_zero_rtt {
        builder = builder.with_enable_zero_rtt(value.clone());
    }

    if let Some(value) = &args.enable_connection_multiplexing {
        builder = builder.with_enable_connection_multiplexing(value.clone());
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
