mod connect;
mod dns;

use clap::{Parser, ValueEnum};
use connect::connection::Builder as ConnectionBuilder;
use connect::stream::Builder as StreamBuilder;
use dns::Resolver;
use ombrac_protocol::server::Server;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Server listening address
    #[arg(short, long, value_name = "ADDRESS")]
    listen: String,

    /// TLS certificate file path
    #[arg(long, value_name = "PATH")]
    tls_cert: String,

    /// TLS private key file path
    #[arg(long, value_name = "PATH")]
    tls_key: String,

    /// DNS resolver
    #[arg(long, default_value = "cloudflare", value_name = "ENUM")]
    domain_name_system: DomainNameSystem,

    /// Initial congestion window size in bytes
    #[arg(long, default_value = None, value_name = "VALUE")]
    initial_congestion_window: Option<u32>,
}

#[derive(Debug, Clone, ValueEnum)]
enum DomainNameSystem {
    Cloudflare,
    CloudflareTLS,
    Google,
    GoogleTLS,
    Quad9,
    Quad9TLS,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let server = {
        use std::path::Path;

        use s2n_quic::provider::congestion_controller;
        use s2n_quic::Server as NoiseServer;

        let controller = {
            let controller = congestion_controller::bbr::Builder::default();
            let controller = match args.initial_congestion_window {
                Some(value) => controller.with_initial_congestion_window(value),
                None => controller,
            };
            controller.build()
        };

        NoiseServer::builder()
            .with_io(args.listen.as_str())?
            .with_congestion_controller(controller)?
            .with_tls((
                Path::new(args.tls_cert.as_str()),
                Path::new(args.tls_key.as_str()),
            ))?
            .start()?
    };

    let dns = {
        use hickory_resolver::config::{ResolverConfig, ResolverOpts};

        let name_server = match args.domain_name_system {
            DomainNameSystem::Cloudflare => ResolverConfig::cloudflare(),
            DomainNameSystem::CloudflareTLS => ResolverConfig::cloudflare_tls(),
            DomainNameSystem::Google => ResolverConfig::google(),
            DomainNameSystem::GoogleTLS => ResolverConfig::google_tls(),
            DomainNameSystem::Quad9 => ResolverConfig::quad9(),
            DomainNameSystem::Quad9TLS => ResolverConfig::quad9_tls(),
        };

        let options = ResolverOpts::default();

        Resolver::from((name_server, options))
    };

    let connection = ConnectionBuilder::new(server).build();
    let stream = StreamBuilder::new(connection).build();

    Server::with(stream, dns).start().await;

    Ok(())
}
