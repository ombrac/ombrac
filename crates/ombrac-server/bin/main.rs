use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::builder::Styles;
use clap::builder::styling::{AnsiColor, Style};
use clap::{Parser, ValueEnum};
#[cfg(feature = "datagram")]
use ombrac::server::datagram::UdpHandlerConfig;
use ombrac::server::{SecretValid, Server};
use ombrac_macros::{error, info};
use ombrac_transport::Acceptor;
#[cfg(feature = "transport-quic")]
use ombrac_transport::quic::{
    Congestion, TransportConfig,
    server::{Config, Server as QuicServer},
};

fn styles() -> Styles {
    Styles::styled()
        .header(Style::new().bold().fg_color(Some(AnsiColor::Green.into())))
        .usage(Style::new().bold().fg_color(Some(AnsiColor::Green.into())))
        .literal(Style::new().bold().fg_color(Some(AnsiColor::Cyan.into())))
        .placeholder(Style::new().fg_color(Some(AnsiColor::Cyan.into())))
        .valid(Style::new().bold().fg_color(Some(AnsiColor::Cyan.into())))
        .invalid(Style::new().bold().fg_color(Some(AnsiColor::Yellow.into())))
        .error(Style::new().bold().fg_color(Some(AnsiColor::Red.into())))
}

#[cfg(feature = "transport-quic")]
#[derive(ValueEnum, Clone, Debug, Copy)]
enum TlsMode {
    Tls,
    MTls,
    Insecure,
}

#[derive(Debug, Parser)]
#[command(version, about, long_about = None, styles = styles() )]
struct Args {
    /// Protocol Secret
    #[clap(
        long,
        short = 'k',
        help_heading = "Required",
        value_name = "STR",
        verbatim_doc_comment
    )]
    secret: String,

    /// The address to bind for transport
    #[clap(
        long,
        short = 'l',
        help_heading = "Required",
        value_name = "ADDR",
        verbatim_doc_comment
    )]
    listen: SocketAddr,

    // Transport (QUIC)
    /// Set the TLS mode for the connection
    /// tls: Standard TLS. The client verifies the server's certificate
    /// m-tls: Mutual TLS with client and server certificate verification
    /// insecure: Generates a self-signed certificate on the fly with `SANs` set to `localhost` (for testing only)
    #[cfg(feature = "transport-quic")]
    #[clap(
        long,
        value_enum,
        default_value_t = TlsMode::Tls,
        help_heading = "Transport",
        verbatim_doc_comment
    )]
    tls_mode: TlsMode,

    /// Path to the Certificate Authority (CA) certificate file
    /// Used in 'mTLS' modes
    #[cfg(feature = "transport-quic")]
    #[clap(
        long,
        help_heading = "Transport",
        value_name = "FILE",
        verbatim_doc_comment
    )]
    ca_cert: Option<PathBuf>,

    /// Path to the TLS certificate file
    #[cfg(feature = "transport-quic")]
    #[clap(
        long,
        help_heading = "Transport",
        value_name = "FILE",
        verbatim_doc_comment
    )]
    tls_cert: Option<PathBuf>,

    /// Path to the TLS private key file
    #[cfg(feature = "transport-quic")]
    #[clap(
        long,
        help_heading = "Transport",
        value_name = "FILE",
        verbatim_doc_comment
    )]
    tls_key: Option<PathBuf>,

    /// Enable 0-RTT for faster connection establishment (may reduce security)
    #[clap(long, help_heading = "Transport", action, verbatim_doc_comment)]
    #[cfg(feature = "transport-quic")]
    zero_rtt: bool,

    /// Application-Layer protocol negotiation (ALPN) protocols
    /// e.g. "h3,h3-29"
    #[clap(
        long,
        help_heading = "Transport",
        value_name = "PROTOCOLS",
        default_value = "h3",
        value_delimiter = ',',
        verbatim_doc_comment
    )]
    #[cfg(feature = "transport-quic")]
    alpn_protocols: Vec<Vec<u8>>,

    /// Congestion control algorithm to use (e.g. bbr, cubic, newreno)
    #[cfg(feature = "transport-quic")]
    #[clap(
        long,
        help_heading = "Transport",
        value_name = "ALGORITHM",
        default_value = "bbr",
        verbatim_doc_comment
    )]
    congestion: Congestion,

    /// Initial congestion window size in bytes
    #[cfg(feature = "transport-quic")]
    #[clap(
        long,
        help_heading = "Transport",
        value_name = "NUM",
        verbatim_doc_comment
    )]
    cwnd_init: Option<u64>,

    /// Maximum idle time (in milliseconds) before closing the connection
    /// 30 second default recommended by RFC 9308
    #[cfg(feature = "transport-quic")]
    #[clap(
        long,
        help_heading = "Transport",
        value_name = "TIME",
        default_value = "30000",
        verbatim_doc_comment
    )]
    idle_timeout: u64,

    /// Keep-alive interval (in milliseconds)
    #[cfg(feature = "transport-quic")]
    #[clap(
        long,
        help_heading = "Transport",
        value_name = "TIME",
        default_value = "8000",
        verbatim_doc_comment
    )]
    keep_alive: u64,

    /// Maximum number of bidirectional streams that can be open simultaneously
    #[cfg(feature = "transport-quic")]
    #[clap(
        long,
        help_heading = "Transport",
        value_name = "NUM",
        default_value = "1000",
        verbatim_doc_comment
    )]
    max_streams: u64,

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
        info!("Server Listening on {}", args.listen);
        let server = Server::new(quic_server_from_args(&args).await?);
        run_loop(
            server.into(),
            validator,
            #[cfg(feature = "datagram")]
            udp_config,
        )
        .await?;
    }

    Ok(())
}

fn secret_validator_from_args(args: &Args) -> SecretValid {
    let secret = *blake3::hash(args.secret.as_bytes()).as_bytes();
    SecretValid(secret)
}

async fn run_loop(
    server: Arc<Server<impl Acceptor>>,
    validator: SecretValid,
    #[cfg(feature = "datagram")] udp_config: Arc<UdpHandlerConfig>,
) -> io::Result<()> {
    let connect_handle = {
        let server = Arc::clone(&server);
        tokio::spawn(async move {
            loop {
                match server.accept_connect().await {
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
        let server = Arc::clone(&server);
        tokio::spawn(async move {
            loop {
                let config = Arc::clone(&udp_config);
                match server.accept_associate().await {
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
        datagram??;
    }

    #[cfg(not(feature = "datagram"))]
    connect_handle.await??;

    Ok(())
}

#[cfg(feature = "transport-quic")]
async fn quic_server_from_args(args: &Args) -> io::Result<QuicServer> {
    let mut config = Config::new();

    config.enable_zero_rtt = args.zero_rtt;
    config.alpn_protocols = args.alpn_protocols.clone();
    match args.tls_mode {
        TlsMode::Tls => {
            if let (Some(cert), Some(key)) = (&args.tls_cert, &args.tls_key) {
                config.tls_cert_key_paths = Some((cert.to_path_buf(), key.to_path_buf()));
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "--tls-cert and --tls-key are required for TLS mode",
                ));
            }
        }
        TlsMode::MTls => {
            if let (Some(cert), Some(key)) = (&args.tls_cert, &args.tls_key) {
                config.tls_cert_key_paths = Some((cert.to_path_buf(), key.to_path_buf()));
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "--tls-cert and --tls-key are required for mTLS mode",
                ));
            }

            if let Some(ca_cert) = &args.ca_cert {
                config.root_ca_path = Some(ca_cert.to_path_buf());
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "--ca-cert is required for mTLS mode",
                ));
            }
        }
        TlsMode::Insecure => {
            config.enable_self_signed = true;
        }
    };

    let mut transport_config = TransportConfig::default();
    transport_config.max_idle_timeout(Duration::from_millis(args.idle_timeout))?;
    transport_config.keep_alive_period(Duration::from_millis(args.keep_alive))?;
    transport_config.max_open_bidirectional_streams(args.max_streams)?;
    transport_config.congestion(args.congestion, args.cwnd_init)?;
    config.transport_config(transport_config);

    let socket = UdpSocket::bind(args.listen)?;
    Ok(QuicServer::new(config, socket).await?)
}
