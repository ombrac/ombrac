use std::error::Error;
use std::net::ToSocketAddrs;
use std::time::Duration;

use clap::{Parser, ValueEnum};
use ombrac_server::dns::Resolver;
use ombrac_server::transport::quic::{Config as QuicConfig, NoiseQuic};
use ombrac_server::Server;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Transport server listening address
    #[clap(short, long, help_heading = "Transport QUIC", value_name = "ADDR")]
    listen: String,

    /// Path to the TLS certificate file for secure connections.
    #[clap(long, help_heading = "Transport QUIC", value_name = "FILE")]
    tls_cert: String,

    /// Path to the TLS private key file for secure connections.
    #[clap(long, help_heading = "Transport QUIC", value_name = "FILE")]
    tls_key: String,

    /// Initial congestion window in bytes
    #[clap(
        long,
        help_heading = "Transport QUIC",
        value_name = "NUM",
        default_value = "32"
    )]
    initial_congestion_window: u32,

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
        default_value = "600000"
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

    /// Domain name system resolver
    #[arg(
        long,
        default_value = "cloudflare",
        value_name = "ENUM",
        help_heading = "DNS"
    )]
    dns: DomainNameSystem,

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

#[derive(Debug, Clone, ValueEnum)]
enum DomainNameSystem {
    Cloudflare,
    CloudflareTLS,
    Google,
    GoogleTLS,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    #[cfg(feature = "tracing")]
    tracing_subscriber::fmt()
        .with_thread_ids(true)
        .with_max_level(args.tracing_level)
        .init();

    let domain_name_system = dns_from_args(&args)?;
    let transport = NoiseQuic::with(quic_config_from_args(&args)?).await?;

    tracing::info!("server listening on {}", args.listen);

    Server::with(transport, domain_name_system).start().await;

    Ok(())
}

fn dns_from_args(args: &Args) -> Result<Resolver, Box<dyn Error>> {
    let name_server = match args.dns {
        DomainNameSystem::Cloudflare => ResolverConfig::cloudflare(),
        DomainNameSystem::CloudflareTLS => ResolverConfig::cloudflare_tls(),
        DomainNameSystem::Google => ResolverConfig::google(),
        DomainNameSystem::GoogleTLS => ResolverConfig::google_tls(),
    };

    use hickory_resolver::config::{ResolverConfig, ResolverOpts};

    let options = ResolverOpts::default();

    Ok(Resolver::from((name_server, options)))
}

fn quic_config_from_args(args: &Args) -> Result<QuicConfig, Box<dyn Error>> {
    let listen = args
        .listen
        .to_socket_addrs()?
        .nth(0)
        .ok_or(format!("unable to resolve address {}", args.listen))?;

    let mut config = QuicConfig::new(listen, args.tls_cert.clone(), args.tls_key.clone());

    config = config
        .with_initial_congestion_window(args.initial_congestion_window)
        .with_max_handshake_duration(Duration::from_millis(args.max_handshake_duration))
        .with_max_idle_timeout(Duration::from_millis(args.max_idle_timeout))
        .with_max_keep_alive_period(Duration::from_millis(args.max_keep_alive_period))
        .with_max_open_bidirectional_streams(args.max_open_bidirectional_streams)
        .with_bidirectional_local_data_window(args.bidirectional_local_data_window)
        .with_bidirectional_remote_data_window(args.bidirectional_remote_data_window);

    Ok(config)
}
