use std::net::ToSocketAddrs;

use clap::Parser;
use ombrac_client::endpoint::socks::SocksServer;
use ombrac_client::transport::quic::connection::Builder as ConnectionBuilder;
use ombrac_client::transport::quic::stream::Builder as StreamBuilder;
use ombrac_client::Client;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Args {
    /// Remote server IP address or domain name
    /// e.g., example.com:port
    #[arg(short, long, verbatim_doc_comment, value_name = "ADDRESS")]
    remote: String,

    /// Listening address for the SOCKS server.
    #[arg(short, long, default_value = "127.0.0.1:1080", value_name = "ADDRESS")]
    listen: String,

    /// IO provider address for the client.
    #[arg(long, default_value = "0.0.0.0:0", value_name = "ADDRESS")]
    bind: String,

    /// TLS SNI (Server Name Indication); defaults to remote address if not provided.
    #[arg(long, default_value = None, value_name = "PATH")]
    tls_sni: Option<String>,

    /// Path to the TLS certificate file.
    #[arg(long, default_value = None, value_name = "PATH")]
    tls_cert: Option<String>,

    /// Initial congestion window size in bytes.
    #[arg(long, default_value = None, value_name = "VALUE")]
    initial_congestion_window: Option<u32>,

    /// Limit on the number of concurrent client instances.
    #[arg(long, default_value = None, value_name = "VALUE")]
    limit_concurrent_instances: Option<usize>,

    /// Limit on the number of connection reuses.
    #[arg(long, default_value = None, value_name = "VALUE")]
    limit_connection_reuses: Option<usize>,

    /// Maximum amount of data that can be sent without acknowledgment.
    #[arg(long, default_value = None, value_name = "VALUE")]
    limit_bidirectional_local_data_window: Option<u64>,

    /// Maximum amount of data the remote peer can send before needing acknowledgment.
    #[arg(long, default_value = None, value_name = "VALUE")]
    limit_bidirectional_remote_data_window: Option<u64>,

    /// Restriction on the number of concurrent streams initiated by the local peer.
    #[arg(long, default_value = None, value_name = "VALUE")]
    limit_max_open_local_bidirectional_streams: Option<u64>,

    /// Restriction on the number of concurrent streams initiated by the remote peer.
    #[arg(long, default_value = None, value_name = "VALUE")]
    limit_max_open_remote_bidirectional_streams: Option<u64>,

    /// Maximum duration (in seconds) allowed for connection establishment before timing out.
    #[arg(long, default_value = "4", value_name = "VALUE")]
    limit_max_handshake_duration: Option<u64>,

    /// Maximum interval (in seconds) for sending keep-alive packets to maintain the connection.
    #[arg(long, default_value = "8", value_name = "VALUE")]
    limit_max_keep_alive_period: Option<u64>,

    /// Logging level
    /// e.g., INFO, WARN, ERROR
    #[cfg(feature = "tracing")]
    #[arg(
        long,
        default_value = "WARN",
        verbatim_doc_comment,
        value_name = "LEVEL"
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

    let client = match args.limit_concurrent_instances {
        Some(num) => (0..num)
            .map(|_| s2n_quic_client::build(&args))
            .collect::<Result<Vec<_>, _>>()?,

        None => vec![s2n_quic_client::build(&args)?],
    };

    let server_name = match args.tls_sni {
        Some(value) => value,
        None => {
            let address = args.remote.clone();
            let pos = address.rfind(':').ok_or("invalid remote address")?;
            address[..pos].to_string()
        }
    };

    let server_address = args
        .remote
        .to_socket_addrs()?
        .nth(0)
        .ok_or(format!("unable to resolve address {}", args.remote))?;

    let connection = ConnectionBuilder::new(client, server_name, server_address).build();
    let stream_builder = StreamBuilder::new(connection);

    let stream_builder = stream_builder.with_connection_reuses(args.limit_connection_reuses);

    let stream = stream_builder.build();

    let socks_server = SocksServer::with(args.listen).await?;

    Client::with(socks_server, stream).start().await;

    Ok(())
}

mod s2n_quic_client {
    use std::error::Error;
    use std::time::Duration;

    use s2n_quic::provider::{congestion_controller, limits};
    use s2n_quic::Client as NoiseClient;
    use std::path::Path;

    use super::Args;

    pub fn build(args: &Args) -> Result<NoiseClient, Box<dyn Error>> {
        let limits = {
            let limits = limits::Limits::new();

            let limits = match args.limit_bidirectional_local_data_window {
                Some(value) => limits.with_bidirectional_local_data_window(value)?,
                None => limits,
            };

            let limits = match args.limit_bidirectional_remote_data_window {
                Some(value) => limits.with_bidirectional_remote_data_window(value)?,
                None => limits,
            };

            let limits = match args.limit_max_open_local_bidirectional_streams {
                Some(value) => limits.with_max_open_local_bidirectional_streams(value)?,
                None => limits,
            };

            let limits = match args.limit_max_open_remote_bidirectional_streams {
                Some(value) => limits.with_max_open_remote_bidirectional_streams(value)?,
                None => limits,
            };

            let limits = match args.limit_max_handshake_duration {
                Some(value) => limits.with_max_handshake_duration(Duration::from_secs(value))?,
                None => limits,
            };

            let limits = match args.limit_max_keep_alive_period {
                Some(value) => limits.with_max_keep_alive_period(Duration::from_secs(value))?,
                None => limits,
            };

            limits
        };

        let controller = {
            let controller = congestion_controller::bbr::Builder::default();
            let controller = match args.initial_congestion_window {
                Some(value) => controller.with_initial_congestion_window(value),
                None => controller,
            };
            controller.build()
        };

        let client = NoiseClient::builder()
            .with_io(args.bind.as_str())?
            .with_limits(limits)?
            .with_congestion_controller(controller)?;

        let client = match &args.tls_cert {
            Some(path) => client.with_tls(Path::new(path.as_str()))?.start()?,
            None => client.start()?,
        };

        Ok(client)
    }
}
