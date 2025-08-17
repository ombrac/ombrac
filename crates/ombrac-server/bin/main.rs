use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
#[cfg(feature = "datagram")]
use ombrac::server::datagram::UdpHandlerConfig;
use ombrac::server::{SecretValid, Server};
use ombrac_macros::{error, info};
use ombrac_transport::Acceptor;
#[cfg(feature = "transport-quic")]
use ombrac_transport::quic::{Congestion, server::Builder};

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

    #[cfg(feature = "transport-quic")]
    /// Congestion control algorithm to use (e.g. bbr, cubic, newreno)
    #[clap(
        long,
        help_heading = "Transport QUIC",
        value_name = "ALGORITHM",
        default_value = "bbr",
        verbatim_doc_comment
    )]
    congestion: Congestion,

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

    /// Maximum idle time for a UDP association (in milliseconds)
    #[cfg(feature = "datagram")]
    #[clap(
        long,
        help_heading = "Transport UDP",
        value_name = "TIME",
        default_value = "30000",
        verbatim_doc_comment
    )]
    udp_idle_timeout: u64,

    /// Buffer size for receiving UDP packets (in bytes)
    #[cfg(feature = "datagram")]
    #[clap(
        long,
        help_heading = "Transport UDP",
        value_name = "BYTES",
        default_value = "1500",
        verbatim_doc_comment
    )]
    udp_buffer_size: usize,

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
async fn main() -> io::Result<()> {
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

    let validator = secret_validator_from_args(&args);

    #[cfg(feature = "datagram")]
    let udp_config = Arc::new(UdpHandlerConfig {
        idle_timeout: Duration::from_millis(args.udp_idle_timeout),
        buffer_size: args.udp_buffer_size,
    });

    #[cfg(feature = "transport-quic")]
    {
        info!("Server listening on {}", args.listen);
        let transport = Arc::new(Server::new(quic_server_from_args(&args).await?));
        tokio::spawn(run_server(
            transport,
            validator,
            #[cfg(feature = "datagram")]
            udp_config,
        )).await??;
    }

    Ok(())
}

fn secret_validator_from_args(args: &Args) -> SecretValid {
    let secret = *blake3::hash(args.secret.as_bytes()).as_bytes();
    SecretValid(secret)
}

#[cfg(feature = "transport-quic")]
async fn quic_server_from_args(args: &Args) -> io::Result<impl Acceptor> {
    let bind_addr = args.listen;
    let mut builder = Builder::new(bind_addr);

    if let Some(cert) = &args.tls_cert
        && let Some(key) = &args.tls_key
    {
        builder.with_tls((cert.to_path_buf(), key.to_path_buf()));
    }

    if let Some(value) = args.idle_timeout {
        builder.with_max_idle_timeout(Duration::from_millis(value))?;
    }

    if let Some(value) = args.keep_alive {
        builder.with_max_keep_alive_period(Duration::from_millis(value));
    }

    if let Some(value) = args.max_streams {
        builder.with_max_open_bidirectional_streams(value)?;
    }

    builder.with_enable_self_signed(args.insecure);
    builder.with_enable_zero_rtt(args.zero_rtt);
    builder.with_congestion(args.congestion, args.cwnd_init);

    Ok(builder.build().await?)
}

async fn run_server(
    transport: Arc<Server<impl Acceptor>>,
    validator: SecretValid,
    #[cfg(feature = "datagram")] udp_config: Arc<UdpHandlerConfig>,
) -> io::Result<()> {
    let connect_handle = {
        let transport = Arc::clone(&transport);
        tokio::spawn(async move {
            loop {
                match transport.accept_connect().await {
                    Ok(stream) => tokio::spawn(async move {
                        if let Err(_error) = Server::handle_connect(&validator, stream).await {
                            error!("{_error}");
                        }
                    }),

                    Err(error) => return Err::<(), io::Error>(error),
                };
            }
        })
    };

    #[cfg(feature = "datagram")]
    let datagram_handle = {
        let transport = Arc::clone(&transport);
        tokio::spawn(async move {
            loop {
                let config = udp_config.clone();
                match transport.accept_associate().await {
                    Ok(datagram) => tokio::spawn(async move {
                        if let Err(_error) =
                            Server::handle_associate(&validator, datagram, config).await
                        {
                            error!("{_error}");
                        }
                    }),

                    Err(error) => return Err::<(), io::Error>(error),
                };
            }
        })
    };

    #[cfg(feature = "datagram")]
    {
        let (connect, datagram) = tokio::join!(connect_handle, datagram_handle);
        connect??;
        datagram??
    }

    #[cfg(not(feature = "datagram"))]
    connect_handle.await??;

    Ok(())
}
