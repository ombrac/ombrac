mod connect;
mod dns;

use clap::Parser;
use connect::connection::Builder as ConnectionBuilder;
use connect::stream::Builder as StreamBuilder;
use dns::Resolver;
use ombrac_protocol::server::Server;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Server listening address
    #[arg(short, long)]
    listen: String,

    /// TLS certificate file path
    #[arg(long)]
    tls_cert: String,

    /// TLS private key file path
    #[arg(long)]
    tls_key: String,

    /// Initial congestion window size in bytes
    #[arg(long, default_value = None)]
    initial_congestion_window: Option<u32>,
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

    let connection = ConnectionBuilder::new(server).build();
    let stream = StreamBuilder::new(connection).build();

    Server::with(stream, Resolver::default()).start().await;

    Ok(())
}
