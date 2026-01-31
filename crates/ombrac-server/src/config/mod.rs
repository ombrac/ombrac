use std::net::SocketAddr;
use std::path::PathBuf;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use ombrac_transport::quic::Congestion;

pub mod cli;
pub mod json;

/// Transport configuration for QUIC connections
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub struct TransportConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_mode: Option<TlsMode>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub ca_cert: Option<PathBuf>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_cert: Option<PathBuf>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_key: Option<PathBuf>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub zero_rtt: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub alpn_protocols: Option<Vec<Vec<u8>>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub congestion: Option<Congestion>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwnd_init: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub idle_timeout: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_streams: Option<u64>,
}

impl TransportConfig {
    /// Get TLS mode with default
    pub fn tls_mode(&self) -> TlsMode {
        self.tls_mode.unwrap_or_default()
    }

    /// Get zero_rtt with default
    pub fn zero_rtt(&self) -> bool {
        self.zero_rtt.unwrap_or(false)
    }

    /// Get ALPN protocols with default
    pub fn alpn_protocols(&self) -> Vec<Vec<u8>> {
        self.alpn_protocols
            .clone()
            .unwrap_or_else(|| vec!["h3".into()])
    }

    /// Get congestion control with default
    pub fn congestion(&self) -> Congestion {
        self.congestion.unwrap_or(Congestion::Bbr)
    }

    /// Get idle timeout with default (in milliseconds)
    pub fn idle_timeout(&self) -> u64 {
        self.idle_timeout.unwrap_or(30000)
    }

    /// Get keep-alive interval with default (in milliseconds)
    pub fn keep_alive(&self) -> u64 {
        self.keep_alive.unwrap_or(8000)
    }

    /// Get max streams with default
    pub fn max_streams(&self) -> u64 {
        self.max_streams.unwrap_or(1000)
    }
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

/// Connection-level configuration for managing connection lifecycle and resource limits
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub struct ConnectionConfig {
    /// Maximum number of concurrent connections [default: 10000]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_connections: Option<usize>,

    /// Authentication timeout in seconds [default: 10]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_timeout_secs: Option<u64>,

    /// Maximum concurrent stream connections per client connection [default: 4096]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_concurrent_streams: Option<usize>,

    /// Maximum concurrent datagram handlers per client connection [default: 4096]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_concurrent_datagrams: Option<usize>,
}

impl ConnectionConfig {
    /// Get max connections with default
    pub fn max_connections(&self) -> usize {
        self.max_connections.unwrap_or(10000)
    }

    /// Get authentication timeout with default (in seconds)
    pub fn auth_timeout_secs(&self) -> u64 {
        self.auth_timeout_secs.unwrap_or(10)
    }

    /// Get handshake timeout with default (in seconds)
    ///
    /// Deprecated: Use `auth_timeout_secs` instead
    #[deprecated(note = "Use auth_timeout_secs instead")]
    pub fn handshake_timeout_secs(&self) -> u64 {
        self.auth_timeout_secs()
    }

    /// Get max concurrent streams with default
    pub fn max_concurrent_streams(&self) -> usize {
        self.max_concurrent_streams.unwrap_or(4096)
    }

    /// Get max concurrent datagrams with default
    pub fn max_concurrent_datagrams(&self) -> usize {
        self.max_concurrent_datagrams.unwrap_or(4096)
    }
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            max_connections: Some(10000),
            auth_timeout_secs: Some(10),
            max_concurrent_streams: Some(4096),
            max_concurrent_datagrams: Some(4096),
        }
    }
}

/// Logging configuration
#[cfg(feature = "tracing")]
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub struct LoggingConfig {
    /// Logging level (e.g., INFO, WARN, ERROR) [default: INFO]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_level: Option<String>,
}

#[cfg(feature = "tracing")]
impl LoggingConfig {
    /// Get log level with default
    pub fn log_level(&self) -> &str {
        self.log_level.as_deref().unwrap_or("INFO")
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

#[derive(ValueEnum, Clone, Debug, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TlsMode {
    #[default]
    Tls,
    MTls,
    Insecure,
}

/// Final service configuration with all defaults applied
#[derive(Debug, Clone)]
pub struct ServiceConfig {
    pub secret: String,
    pub listen: SocketAddr,
    pub transport: TransportConfig,
    pub connection: ConnectionConfig,
    #[cfg(feature = "tracing")]
    pub logging: LoggingConfig,
}

/// Configuration builder that merges different configuration sources
/// and applies defaults in a clear, predictable order
pub struct ConfigBuilder {
    secret: Option<String>,
    listen: Option<SocketAddr>,
    transport: TransportConfig,
    connection: ConnectionConfig,
    #[cfg(feature = "tracing")]
    logging: LoggingConfig,
}

impl ConfigBuilder {
    /// Create a new builder with default values
    pub fn new() -> Self {
        Self {
            secret: None,
            listen: None,
            transport: TransportConfig::default(),
            connection: ConnectionConfig::default(),
            #[cfg(feature = "tracing")]
            logging: LoggingConfig::default(),
        }
    }

    /// Merge JSON configuration into the builder
    pub fn merge_json(mut self, json_config: json::JsonConfig) -> Self {
        if let Some(secret) = json_config.secret {
            self.secret = Some(secret);
        }
        if let Some(listen) = json_config.listen {
            self.listen = Some(listen);
        }
        if let Some(transport) = json_config.transport {
            self.transport = Self::merge_transport(self.transport, transport);
        }
        if let Some(conn) = json_config.connection {
            self.connection = Self::merge_connection(self.connection, conn);
        }
        #[cfg(feature = "tracing")]
        {
            if let Some(logging) = json_config.logging {
                self.logging = Self::merge_logging(self.logging, logging);
            }
        }
        self
    }

    /// Merge CLI configuration into the builder (CLI overrides JSON)
    pub fn merge_cli(mut self, cli_config: cli::CliConfig) -> Self {
        if let Some(secret) = cli_config.secret {
            self.secret = Some(secret);
        }
        if let Some(listen) = cli_config.listen {
            self.listen = Some(listen);
        }
        self.transport = Self::merge_transport(self.transport, cli_config.transport);
        #[cfg(feature = "tracing")]
        {
            self.logging = Self::merge_logging(self.logging, cli_config.logging);
        }
        self
    }

    /// Build the final ServiceConfig, validating required fields
    pub fn build(self) -> Result<ServiceConfig, String> {
        let secret = self
            .secret
            .ok_or_else(|| "missing required field: secret".to_string())?;
        let listen = self
            .listen
            .ok_or_else(|| "missing required field: listen".to_string())?;

        Ok(ServiceConfig {
            secret,
            listen,
            transport: self.transport,
            connection: self.connection,
            #[cfg(feature = "tracing")]
            logging: self.logging,
        })
    }

    fn merge_transport(base: TransportConfig, override_config: TransportConfig) -> TransportConfig {
        TransportConfig {
            tls_mode: override_config.tls_mode.or(base.tls_mode),
            ca_cert: override_config.ca_cert.or(base.ca_cert),
            tls_cert: override_config.tls_cert.or(base.tls_cert),
            tls_key: override_config.tls_key.or(base.tls_key),
            zero_rtt: override_config.zero_rtt.or(base.zero_rtt),
            alpn_protocols: override_config.alpn_protocols.or(base.alpn_protocols),
            congestion: override_config.congestion.or(base.congestion),
            cwnd_init: override_config.cwnd_init.or(base.cwnd_init),
            idle_timeout: override_config.idle_timeout.or(base.idle_timeout),
            keep_alive: override_config.keep_alive.or(base.keep_alive),
            max_streams: override_config.max_streams.or(base.max_streams),
        }
    }

    fn merge_connection(
        base: ConnectionConfig,
        override_config: ConnectionConfig,
    ) -> ConnectionConfig {
        ConnectionConfig {
            max_connections: override_config.max_connections.or(base.max_connections),
            auth_timeout_secs: override_config.auth_timeout_secs.or(base.auth_timeout_secs),
            max_concurrent_streams: override_config
                .max_concurrent_streams
                .or(base.max_concurrent_streams),
            max_concurrent_datagrams: override_config
                .max_concurrent_datagrams
                .or(base.max_concurrent_datagrams),
        }
    }

    #[cfg(feature = "tracing")]
    fn merge_logging(base: LoggingConfig, override_config: LoggingConfig) -> LoggingConfig {
        LoggingConfig {
            log_level: override_config.log_level.or(base.log_level),
        }
    }
}

impl Default for ConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Load configuration from command-line arguments and/or JSON file.
///
/// This function merges configurations in the following order:
/// 1. Default configuration values
/// 2. Values from JSON config file (if provided)
/// 3. Command-line argument overrides
///
/// # Returns
///
/// A `ServiceConfig` ready to use, or an error if required fields are missing.
#[cfg(feature = "binary")]
pub fn load() -> Result<ServiceConfig, Box<dyn std::error::Error>> {
    use clap::Parser;
    let cli_args = cli::Args::parse();
    let mut builder = ConfigBuilder::new();

    // Load JSON config if specified
    if let Some(config_path) = &cli_args.config {
        let json_config = json::JsonConfig::from_file(config_path)?;
        builder = builder.merge_json(json_config);
    }

    // Merge CLI overrides
    let cli_config = cli::CliConfig {
        secret: cli_args.secret,
        listen: cli_args.listen,
        transport: cli_args.transport.into_transport_config(),
        #[cfg(feature = "tracing")]
        logging: cli_args.logging.into_logging_config(),
    };
    builder = builder.merge_cli(cli_config);

    builder.build().map_err(|e| e.into())
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
pub fn load_from_json(json_str: &str) -> Result<ServiceConfig, Box<dyn std::error::Error>> {
    let json_config = json::JsonConfig::from_json_str(json_str)?;
    ConfigBuilder::new()
        .merge_json(json_config)
        .build()
        .map_err(|e| e.into())
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
pub fn load_from_file(
    config_path: &std::path::Path,
) -> Result<ServiceConfig, Box<dyn std::error::Error>> {
    let json_config = json::JsonConfig::from_file(config_path)?;
    ConfigBuilder::new()
        .merge_json(json_config)
        .build()
        .map_err(|e| e.into())
}
