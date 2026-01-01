use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;
use clap::builder::Styles;
use clap::builder::styling::{AnsiColor, Style};

use ombrac_transport::quic::Congestion;

use crate::config::{TlsMode, TransportConfig};

/// Command-line arguments for the ombrac server
#[derive(Parser, Debug)]
#[command(version, about, long_about = None, styles = styles())]
pub struct Args {
    /// Path to the JSON configuration file
    /// If provided, other required options can be omitted
    #[clap(long, short = 'c', value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// Protocol secret key for client authentication
    /// Must match the secret used by connecting clients
    #[clap(
        long,
        short = 'k',
        help_heading = "Required",
        value_name = "SECRET",
        required_unless_present = "config"
    )]
    pub secret: Option<String>,

    /// Network address and port to bind the server
    /// Format: IP:PORT (e.g., 0.0.0.0:8080 or [::]:8080)
    #[clap(
        long,
        short = 'l',
        help_heading = "Required",
        value_name = "ADDRESS",
        required_unless_present = "config"
    )]
    pub listen: Option<SocketAddr>,

    #[clap(flatten)]
    pub transport: CliTransportConfig,

    #[cfg(feature = "tracing")]
    #[clap(flatten)]
    pub logging: CliLoggingConfig,
}

/// CLI-specific transport configuration
#[derive(Parser, Debug, Clone)]
pub struct CliTransportConfig {
    /// TLS authentication mode
    ///
    /// - tls: Standard TLS (server certificate only)
    /// - m-tls: Mutual TLS (client and server certificates)
    /// - insecure: Self-signed certificate for testing (localhost only)
    #[clap(long, value_enum, help_heading = "Transport")]
    pub tls_mode: Option<TlsMode>,

    /// Path to the Certificate Authority (CA) certificate file
    /// Required when using m-tls mode
    #[clap(long, help_heading = "Transport", value_name = "FILE")]
    pub ca_cert: Option<PathBuf>,

    /// Path to the TLS server certificate file (PEM format)
    /// Required for tls and m-tls modes
    #[clap(long, help_heading = "Transport", value_name = "FILE")]
    pub tls_cert: Option<PathBuf>,

    /// Path to the TLS private key file (PEM format)
    /// Required for tls and m-tls modes
    #[clap(long, help_heading = "Transport", value_name = "FILE")]
    pub tls_key: Option<PathBuf>,

    /// Enable 0-RTT (Zero Round-Trip Time) for faster connection establishment
    /// Warning: May reduce security by allowing replay attacks
    #[clap(long, help_heading = "Transport", action)]
    pub zero_rtt: Option<bool>,

    /// ALPN (Application-Layer Protocol Negotiation) protocols
    /// Comma-separated list (e.g., "h3,h2") [default: h3]
    #[clap(
        long,
        help_heading = "Transport",
        value_name = "PROTOCOLS",
        value_delimiter = ','
    )]
    pub alpn_protocols: Option<Vec<Vec<u8>>>,

    /// Congestion control algorithm
    /// Options: bbr, cubic, newreno [default: bbr]
    #[clap(long, help_heading = "Transport", value_name = "ALGORITHM")]
    pub congestion: Option<Congestion>,

    /// Initial congestion window size in bytes
    /// Leave unset to use algorithm default
    #[clap(long, help_heading = "Transport", value_name = "BYTES")]
    pub cwnd_init: Option<u64>,

    /// Maximum idle timeout in milliseconds
    /// Connection will be closed if no activity for this duration [default: 30000]
    #[clap(long, help_heading = "Transport", value_name = "MS")]
    pub idle_timeout: Option<u64>,

    /// Keep-alive ping interval in milliseconds
    /// Sends periodic pings to detect dead connections [default: 8000]
    #[clap(long, help_heading = "Transport", value_name = "MS")]
    pub keep_alive: Option<u64>,

    /// Maximum number of concurrent bidirectional streams per connection
    /// Limits resource usage per client [default: 1000]
    #[clap(long, help_heading = "Transport", value_name = "COUNT")]
    pub max_streams: Option<u64>,
}

/// CLI-specific logging configuration
#[cfg(feature = "tracing")]
#[derive(Parser, Debug, Clone)]
pub struct CliLoggingConfig {
    /// Logging verbosity level
    /// Options: TRACE, DEBUG, INFO, WARN, ERROR [default: INFO]
    #[clap(long, help_heading = "Logging", value_name = "LEVEL")]
    pub log_level: Option<String>,
}

impl CliTransportConfig {
    /// Convert CLI transport config to internal TransportConfig
    pub fn into_transport_config(self) -> TransportConfig {
        TransportConfig {
            tls_mode: self.tls_mode,
            ca_cert: self.ca_cert,
            tls_cert: self.tls_cert,
            tls_key: self.tls_key,
            zero_rtt: self.zero_rtt,
            alpn_protocols: self.alpn_protocols,
            congestion: self.congestion,
            cwnd_init: self.cwnd_init,
            idle_timeout: self.idle_timeout,
            keep_alive: self.keep_alive,
            max_streams: self.max_streams,
        }
    }
}

#[cfg(feature = "tracing")]
impl CliLoggingConfig {
    /// Convert CLI logging config to internal LoggingConfig
    pub fn into_logging_config(self) -> crate::config::LoggingConfig {
        crate::config::LoggingConfig {
            log_level: self.log_level,
        }
    }
}

/// Represents CLI arguments as a partial configuration
#[derive(Debug)]
pub struct CliConfig {
    pub secret: Option<String>,
    pub listen: Option<SocketAddr>,
    pub transport: TransportConfig,
    #[cfg(feature = "tracing")]
    pub logging: crate::config::LoggingConfig,
}

impl Args {
    /// Parse CLI arguments and convert to CliConfig
    pub fn parse_to_config() -> CliConfig {
        let args = Self::parse();
        CliConfig {
            secret: args.secret,
            listen: args.listen,
            transport: args.transport.into_transport_config(),
            #[cfg(feature = "tracing")]
            logging: args.logging.into_logging_config(),
        }
    }

    /// Get the config file path if specified
    pub fn config_path() -> Option<PathBuf> {
        Self::parse().config
    }
}

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
