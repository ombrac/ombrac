use std::net::SocketAddr;
use std::path::PathBuf;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use ombrac_transport::quic::Congestion;

pub mod cli;
pub mod json;

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct EndpointConfig {
    /// The address to bind for the HTTP/HTTPS server
    #[cfg(feature = "endpoint-http")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http: Option<SocketAddr>,

    /// The address to bind for the SOCKS server
    #[cfg(feature = "endpoint-socks")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub socks: Option<SocketAddr>,

    #[cfg(feature = "endpoint-tun")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tun: Option<TunConfig>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub struct TransportConfig {
    /// The address to bind for transport
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bind: Option<SocketAddr>,

    /// Name of the server to connect (derived from `server` if not provided)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_name: Option<String>,

    /// Set the TLS mode for the connection
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_mode: Option<TlsMode>,

    /// Path to the Certificate Authority (CA) certificate file
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ca_cert: Option<PathBuf>,

    /// Path to the client's TLS certificate for mTLS
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_cert: Option<PathBuf>,

    /// Path to the client's TLS private key for mTLS
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_key: Option<PathBuf>,

    /// Enable 0-RTT for faster connection establishment (may reduce security)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zero_rtt: Option<bool>,

    /// Application-Layer protocol negotiation (ALPN) protocols [default: h3]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alpn_protocols: Option<Vec<Vec<u8>>>,

    /// Congestion control algorithm to use (e.g. bbr, cubic, newreno) [default: bbr]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub congestion: Option<Congestion>,

    /// Initial congestion window size in bytes
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwnd_init: Option<u64>,

    /// Maximum idle time (in milliseconds) before closing the connection [default: 30000]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idle_timeout: Option<u64>,

    /// Keep-alive interval (in milliseconds) [default: 8000]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<u64>,

    /// Maximum number of bidirectional streams that can be open simultaneously [default: 100]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_streams: Option<u64>,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            bind: None,
            server_name: None,
            tls_mode: Some(TlsMode::Tls),
            ca_cert: None,
            client_cert: None,
            client_key: None,
            zero_rtt: Some(false),
            alpn_protocols: Some(vec!["h3".into()]),
            congestion: Some(Congestion::Bbr),
            cwnd_init: None,
            idle_timeout: Some(30000),
            keep_alive: Some(8000),
            max_streams: Some(100),
        }
    }
}

#[cfg(feature = "tracing")]
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub struct LoggingConfig {
    /// Logging level (e.g., INFO, WARN, ERROR) [default: INFO]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_level: Option<String>,
}

#[cfg(feature = "tracing")]
impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            log_level: Some("INFO".to_string()),
        }
    }
}

#[cfg(feature = "endpoint-tun")]
#[derive(Deserialize, Serialize, Debug, Clone)]
#[cfg_attr(
    all(feature = "binary", feature = "endpoint-tun"),
    derive(clap::Parser)
)]
pub struct TunConfig {
    /// Use a pre-existing TUN device by providing its file descriptor.  
    /// `tun_ipv4`, `tun_ipv6`, and `tun_mtu` will be ignored.
    #[cfg_attr(
        feature = "binary",
        clap(long, help_heading = "Endpoint", value_name = "FD")
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tun_fd: Option<i32>,

    /// The IPv4 address and subnet for the TUN device, in CIDR notation (e.g., 198.19.0.1/24).
    #[cfg_attr(
        feature = "binary",
        clap(long, help_heading = "Endpoint", value_name = "CIDR")
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tun_ipv4: Option<String>,

    /// The IPv6 address and subnet for the TUN device, in CIDR notation (e.g., fd00::1/64).
    #[cfg_attr(
        feature = "binary",
        clap(long, help_heading = "Endpoint", value_name = "CIDR")
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tun_ipv6: Option<String>,

    /// The Maximum Transmission Unit (MTU) for the TUN device. [default: 1500]
    #[cfg_attr(
        feature = "binary",
        clap(long, help_heading = "Endpoint", value_name = "U16")
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tun_mtu: Option<u16>,

    /// The IPv4 address pool for the built-in fake DNS server, in CIDR notation. [default: 198.18.0.0/16]
    #[cfg_attr(
        feature = "binary",
        clap(long, help_heading = "Endpoint", value_name = "CIDR")
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fake_dns: Option<String>,
}

#[cfg(feature = "endpoint-tun")]
impl Default for TunConfig {
    fn default() -> Self {
        Self {
            tun_fd: None,
            tun_ipv4: None,
            tun_ipv6: None,
            tun_mtu: Some(1500),
            fake_dns: Some("198.18.0.0/16".to_string()),
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
    pub server: String,
    pub auth_option: Option<String>,
    pub endpoint: EndpointConfig,
    pub transport: TransportConfig,
    #[cfg(feature = "tracing")]
    pub logging: LoggingConfig,
}

/// Configuration builder that merges different configuration sources
/// and applies defaults in a clear, predictable order
pub struct ConfigBuilder {
    secret: Option<String>,
    server: Option<String>,
    auth_option: Option<String>,
    endpoint: EndpointConfig,
    transport: TransportConfig,
    #[cfg(feature = "tracing")]
    logging: LoggingConfig,
}

impl ConfigBuilder {
    /// Create a new builder with default values
    pub fn new() -> Self {
        Self {
            secret: None,
            server: None,
            auth_option: None,
            endpoint: EndpointConfig::default(),
            transport: TransportConfig::default(),
            #[cfg(feature = "tracing")]
            logging: LoggingConfig::default(),
        }
    }

    /// Merge JSON configuration into the builder
    pub fn merge_json(mut self, json_config: json::JsonConfig) -> Self {
        if let Some(secret) = json_config.secret {
            self.secret = Some(secret);
        }
        if let Some(server) = json_config.server {
            self.server = Some(server);
        }
        if let Some(auth_option) = json_config.auth_option {
            self.auth_option = Some(auth_option);
        }
        if let Some(endpoint) = json_config.endpoint {
            self.endpoint = Self::merge_endpoint(self.endpoint, endpoint);
        }
        if let Some(transport) = json_config.transport {
            self.transport = Self::merge_transport(self.transport, transport);
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
        if let Some(server) = cli_config.server {
            self.server = Some(server);
        }
        if let Some(auth_option) = cli_config.auth_option {
            self.auth_option = Some(auth_option);
        }
        self.endpoint = Self::merge_endpoint(self.endpoint, cli_config.endpoint);
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
        let server = self
            .server
            .ok_or_else(|| "missing required field: server".to_string())?;

        Ok(ServiceConfig {
            secret,
            server,
            auth_option: self.auth_option,
            endpoint: self.endpoint,
            transport: self.transport,
            #[cfg(feature = "tracing")]
            logging: self.logging,
        })
    }

    fn merge_endpoint(_base: EndpointConfig, _override_config: EndpointConfig) -> EndpointConfig {
        EndpointConfig {
            #[cfg(feature = "endpoint-http")]
            http: _override_config.http.or(_base.http),
            #[cfg(feature = "endpoint-socks")]
            socks: _override_config.socks.or(_base.socks),
            #[cfg(feature = "endpoint-tun")]
            tun: _override_config.tun.or(_base.tun),
        }
    }

    fn merge_transport(base: TransportConfig, override_config: TransportConfig) -> TransportConfig {
        TransportConfig {
            bind: override_config.bind.or(base.bind),
            server_name: override_config.server_name.or(base.server_name),
            tls_mode: override_config.tls_mode.or(base.tls_mode),
            ca_cert: override_config.ca_cert.or(base.ca_cert),
            client_cert: override_config.client_cert.or(base.client_cert),
            client_key: override_config.client_key.or(base.client_key),
            zero_rtt: override_config.zero_rtt.or(base.zero_rtt),
            alpn_protocols: override_config.alpn_protocols.or(base.alpn_protocols),
            congestion: override_config.congestion.or(base.congestion),
            cwnd_init: override_config.cwnd_init.or(base.cwnd_init),
            idle_timeout: override_config.idle_timeout.or(base.idle_timeout),
            keep_alive: override_config.keep_alive.or(base.keep_alive),
            max_streams: override_config.max_streams.or(base.max_streams),
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
        server: cli_args.server,
        auth_option: cli_args.auth_option,
        endpoint: cli_args.endpoint.into_endpoint_config(),
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
    let json_config = json::JsonConfig::from_str(json_str)?;
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
