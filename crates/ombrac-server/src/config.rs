use std::net::SocketAddr;
use std::path::PathBuf;

use clap::builder::Styles;
use clap::builder::styling::{AnsiColor, Style};
use clap::{Parser, ValueEnum};

use serde::{Deserialize, Serialize};

use ombrac_transport::quic::Congestion;

// CLI Args
#[derive(Parser, Debug)]
#[command(version, about, long_about = None, styles = styles())]
pub struct Args {
    /// Path to the JSON configuration file
    #[clap(long, short = 'c', value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// Protocol Secret
    #[clap(
        long,
        short = 'k',
        help_heading = "Required",
        value_name = "STR",
        required_unless_present = "config"
    )]
    pub secret: Option<String>,

    /// The address to bind for transport
    #[clap(
        long,
        short = 'l',
        help_heading = "Required",
        value_name = "ADDR",
        required_unless_present = "config"
    )]
    pub listen: Option<SocketAddr>,

    #[clap(flatten)]
    pub transport: TransportConfig,

    #[cfg(feature = "tracing")]
    #[clap(flatten)]
    pub logging: LoggingConfig,
}

// JSON Config File
#[derive(Deserialize, Serialize, Debug, Default)]
pub struct ConfigFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub listen: Option<SocketAddr>,

    pub transport: TransportConfig,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection: Option<ConnectionConfig>,

    #[cfg(feature = "tracing")]
    pub logging: LoggingConfig,
}

#[derive(Deserialize, Serialize, Debug, Parser, Clone)]
pub struct TransportConfig {
    /// Set the TLS mode for the connection
    /// tls: Standard TLS with server certificate verification
    /// m-tls: Mutual TLS with client and server certificate verification
    /// insecure: Generates a self-signed certificate for testing (SANs set to 'localhost')
    #[clap(long, value_enum, help_heading = "Transport")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_mode: Option<TlsMode>,

    /// Path to the Certificate Authority (CA) certificate file for mTLS
    #[clap(long, help_heading = "Transport", value_name = "FILE")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ca_cert: Option<PathBuf>,

    /// Path to the TLS certificate file
    #[clap(long, help_heading = "Transport", value_name = "FILE")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_cert: Option<PathBuf>,

    /// Path to the TLS private key file
    #[clap(long, help_heading = "Transport", value_name = "FILE")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_key: Option<PathBuf>,

    /// Enable 0-RTT for faster connection establishment (may reduce security)
    #[clap(long, help_heading = "Transport", action)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zero_rtt: Option<bool>,

    /// Application-Layer protocol negotiation (ALPN) protocols [default: h3]
    #[clap(
        long,
        help_heading = "Transport",
        value_name = "PROTOCOLS",
        value_delimiter = ','
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alpn_protocols: Option<Vec<Vec<u8>>>,

    /// Congestion control algorithm to use (e.g. bbr, cubic, newreno) [default: bbr]
    #[clap(long, help_heading = "Transport", value_name = "ALGORITHM")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub congestion: Option<Congestion>,

    /// Initial congestion window size in bytes
    #[clap(long, help_heading = "Transport", value_name = "NUM")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwnd_init: Option<u64>,

    /// Maximum idle time (in milliseconds) before closing the connection [default: 30000]
    #[clap(long, help_heading = "Transport", value_name = "TIME")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idle_timeout: Option<u64>,

    /// Keep-alive interval (in milliseconds) [default: 8000]
    #[clap(long, help_heading = "Transport", value_name = "TIME")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<u64>,

    /// Maximum number of bidirectional streams that can be open simultaneously [default: 1000]
    #[clap(long, help_heading = "Transport", value_name = "NUM")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_streams: Option<u64>,
}

/// Connection-level configuration for managing connection lifecycle and resource limits
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct ConnectionConfig {
    /// Maximum number of concurrent connections [default: 10000]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_connections: Option<usize>,

    /// Handshake timeout in seconds [default: 10]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handshake_timeout_secs: Option<u64>,

    /// Maximum concurrent stream connections per client connection [default: 4096]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_concurrent_streams: Option<usize>,

    /// Maximum concurrent datagram handlers per client connection [default: 4096]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_concurrent_datagrams: Option<usize>,
}

#[cfg(feature = "tracing")]
#[derive(Deserialize, Serialize, Debug, Parser, Clone)]
pub struct LoggingConfig {
    /// Logging level (e.g., INFO, WARN, ERROR) [default: INFO]
    #[clap(long, help_heading = "Logging", value_name = "LEVEL")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_level: Option<String>,
}

#[derive(ValueEnum, Clone, Debug, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TlsMode {
    #[default]
    Tls,
    MTls,
    Insecure,
}

#[derive(Debug, Clone)]
pub struct ServiceConfig {
    pub secret: String,
    pub listen: SocketAddr,
    pub transport: TransportConfig,
    pub connection: ConnectionConfig,
    #[cfg(feature = "tracing")]
    pub logging: LoggingConfig,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            tls_mode: Some(TlsMode::Tls),
            ca_cert: None,
            tls_cert: None,
            tls_key: None,
            zero_rtt: Some(false),
            alpn_protocols: Some(vec!["h3".into()]),
            congestion: Some(Congestion::Bbr),
            cwnd_init: None,
            idle_timeout: Some(30000),
            keep_alive: Some(8000),
            max_streams: Some(1000),
        }
    }
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            max_connections: Some(10000),
            handshake_timeout_secs: Some(10),
            max_concurrent_streams: Some(4096),
            max_concurrent_datagrams: Some(4096),
        }
    }
}

#[cfg(feature = "tracing")]
impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            log_level: Some("INFO".to_string()),
        }
    }
}

/// Loads configuration from command-line arguments and/or JSON file.
///
/// This function is intended for use in binary applications. It merges:
/// 1. Default configuration values
/// 2. Values from JSON config file (if provided)
/// 3. Command-line argument overrides
///
/// # Returns
///
/// A `ServiceConfig` ready to use, or an error if required fields are missing.
#[cfg(feature = "binary")]
pub fn load() -> Result<ServiceConfig, Box<figment::Error>> {
    use figment::Figment;
    use figment::providers::{Format, Json, Serialized};

    let args = Args::parse();

    let mut figment = Figment::new().merge(Serialized::defaults(ConfigFile::default()));

    if let Some(config_path) = &args.config {
        if !config_path.exists() {
            let err = std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Configuration file not found: {}", config_path.display()),
            );
            return Err(Box::new(figment::Error::from(err.to_string())));
        }

        figment = figment.merge(Json::file(config_path));
    }

    let cli_overrides = ConfigFile {
        secret: args.secret,
        listen: args.listen,
        transport: args.transport,
        connection: None,
        #[cfg(feature = "tracing")]
        logging: args.logging,
    };

    figment = figment.merge(Serialized::defaults(cli_overrides));

    let config: ConfigFile = figment.extract()?;

    let secret = config
        .secret
        .ok_or_else(|| figment::Error::from("missing field `secret`"))?;
    let listen = config
        .listen
        .ok_or_else(|| figment::Error::from("missing field `listen`"))?;

    Ok(ServiceConfig {
        secret,
        listen,
        transport: config.transport,
        connection: config.connection.unwrap_or_default(),
        #[cfg(feature = "tracing")]
        logging: config.logging,
    })
}

/// Loads configuration from a JSON string.
///
/// This function is useful for programmatic configuration or when loading
/// from external sources (e.g., environment variables, API responses).
///
/// # Arguments
///
/// * `json_str` - A JSON string containing the configuration
///
/// # Returns
///
/// A `ServiceConfig` ready to use, or an error if parsing fails or required fields are missing.
pub fn load_from_json(json_str: &str) -> Result<ServiceConfig, Box<figment::Error>> {
    use figment::Figment;
    use figment::providers::{Format, Json, Serialized};

    let config: ConfigFile = Figment::new()
        .merge(Serialized::defaults(ConfigFile::default()))
        .merge(Json::string(json_str))
        .extract()?;

    let secret = config
        .secret
        .ok_or_else(|| figment::Error::from("missing field `secret` in JSON config"))?;
    let listen = config
        .listen
        .ok_or_else(|| figment::Error::from("missing field `listen` in JSON config"))?;

    Ok(ServiceConfig {
        secret,
        listen,
        transport: config.transport,
        connection: config.connection.unwrap_or_default(),
        #[cfg(feature = "tracing")]
        logging: config.logging,
    })
}

/// Loads configuration from a JSON file.
///
/// This function reads configuration from a file path.
///
/// # Arguments
///
/// * `config_path` - Path to the JSON configuration file
///
/// # Returns
///
/// A `ServiceConfig` ready to use, or an error if the file doesn't exist,
/// parsing fails, or required fields are missing.
pub fn load_from_file(config_path: &std::path::Path) -> Result<ServiceConfig, Box<figment::Error>> {
    use figment::Figment;
    use figment::providers::{Format, Json, Serialized};

    if !config_path.exists() {
        let err = std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Configuration file not found: {}", config_path.display()),
        );
        return Err(Box::new(figment::Error::from(err.to_string())));
    }

    let config: ConfigFile = Figment::new()
        .merge(Serialized::defaults(ConfigFile::default()))
        .merge(Json::file(config_path))
        .extract()?;

    let secret = config
        .secret
        .ok_or_else(|| figment::Error::from("missing field `secret` in config file"))?;
    let listen = config
        .listen
        .ok_or_else(|| figment::Error::from("missing field `listen` in config file"))?;

    Ok(ServiceConfig {
        secret,
        listen,
        transport: config.transport,
        connection: config.connection.unwrap_or_default(),
        #[cfg(feature = "tracing")]
        logging: config.logging,
    })
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
