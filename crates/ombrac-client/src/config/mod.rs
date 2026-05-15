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

    /// Enable 0-RTT for faster connection establishment
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
pub struct TunConfig {
    /// Use a pre-existing TUN device by providing its file descriptor.  
    /// `tun_ipv4`, `tun_ipv6`, and `tun_mtu` will be ignored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tun_fd: Option<i32>,

    /// The IPv4 address and subnet for the TUN device, in CIDR notation (e.g., 198.19.0.1/24).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tun_ipv4: Option<String>,

    /// The IPv6 address and subnet for the TUN device, in CIDR notation (e.g., fd00::1/64).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tun_ipv6: Option<String>,

    /// The Maximum Transmission Unit (MTU) for the TUN device. [default: 1500]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tun_mtu: Option<u16>,

    /// The IPv4 address pool for the built-in fake DNS server, in CIDR notation. [default: 198.18.0.0/16]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fake_dns: Option<String>,

    /// Disable UDP traffic to port 443
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_udp_443: Option<bool>,
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
            disable_udp_443: Some(false),
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
            tun: Self::merge_tun(_base.tun, _override_config.tun),
        }
    }

    #[cfg(feature = "endpoint-tun")]
    fn merge_tun(base: Option<TunConfig>, override_config: Option<TunConfig>) -> Option<TunConfig> {
        match (base, override_config) {
            (None, None) => None,
            (Some(base), None) => Some(base),
            (None, Some(override_config)) => Some(override_config),
            (Some(base), Some(override_config)) => Some(TunConfig {
                tun_fd: override_config.tun_fd.or(base.tun_fd),
                tun_ipv4: override_config.tun_ipv4.or(base.tun_ipv4),
                tun_ipv6: override_config.tun_ipv6.or(base.tun_ipv6),
                tun_mtu: override_config.tun_mtu.or(base.tun_mtu),
                fake_dns: override_config.fake_dns.or(base.fake_dns),
                disable_udp_443: override_config.disable_udp_443.or(base.disable_udp_443),
            }),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_json() -> &'static str {
        r#"{
            "secret": "topsecret",
            "server": "example.com:443"
        }"#
    }

    #[test]
    fn load_from_json_with_only_required_fields_uses_defaults() {
        let cfg = load_from_json(minimal_json()).unwrap();
        assert_eq!(cfg.secret, "topsecret");
        assert_eq!(cfg.server, "example.com:443");
        // Transport defaults applied
        assert_eq!(cfg.transport.tls_mode, Some(TlsMode::Tls));
        assert_eq!(cfg.transport.idle_timeout, Some(30000));
        assert_eq!(cfg.transport.keep_alive, Some(8000));
        assert_eq!(cfg.transport.max_streams, Some(100));
        assert_eq!(cfg.transport.zero_rtt, Some(false));
    }

    #[test]
    fn load_from_json_missing_secret_fails() {
        let json = r#"{ "server": "example.com:443" }"#;
        let err = load_from_json(json).unwrap_err();
        assert!(err.to_string().contains("secret"));
    }

    #[test]
    fn load_from_json_missing_server_fails() {
        let json = r#"{ "secret": "s" }"#;
        let err = load_from_json(json).unwrap_err();
        assert!(err.to_string().contains("server"));
    }

    #[test]
    fn load_from_json_invalid_json_returns_error() {
        let result = load_from_json("not json {");
        assert!(result.is_err());
    }

    #[test]
    fn load_from_json_overrides_transport_defaults() {
        let json = r#"{
            "secret": "k",
            "server": "1.2.3.4:443",
            "transport": {
                "tls_mode": "insecure",
                "idle_timeout": 60000,
                "keep_alive": 4000,
                "max_streams": 200,
                "zero_rtt": true
            }
        }"#;
        let cfg = load_from_json(json).unwrap();
        assert_eq!(cfg.transport.tls_mode, Some(TlsMode::Insecure));
        assert_eq!(cfg.transport.idle_timeout, Some(60000));
        assert_eq!(cfg.transport.keep_alive, Some(4000));
        assert_eq!(cfg.transport.max_streams, Some(200));
        assert_eq!(cfg.transport.zero_rtt, Some(true));
    }

    #[test]
    fn cli_overrides_json_in_merge_order() {
        let json = json::JsonConfig {
            secret: Some("from_json".into()),
            server: Some("from_json:1".into()),
            auth_option: None,
            endpoint: None,
            transport: Some(TransportConfig {
                idle_timeout: Some(11111),
                keep_alive: Some(2222),
                ..Default::default()
            }),
            #[cfg(feature = "tracing")]
            logging: None,
        };

        let cli = cli::CliConfig {
            secret: Some("from_cli".into()),
            server: None, // CLI doesn't override → JSON wins
            auth_option: None,
            endpoint: EndpointConfig::default(),
            transport: TransportConfig {
                idle_timeout: Some(99999),
                keep_alive: None, // CLI doesn't override this one
                ..Default::default()
            },
            #[cfg(feature = "tracing")]
            logging: LoggingConfig::default(),
        };

        let cfg = ConfigBuilder::new()
            .merge_json(json)
            .merge_cli(cli)
            .build()
            .unwrap();

        assert_eq!(cfg.secret, "from_cli"); // CLI wins
        assert_eq!(cfg.server, "from_json:1"); // JSON wins (CLI absent)
        assert_eq!(cfg.transport.idle_timeout, Some(99999)); // CLI wins
        assert_eq!(cfg.transport.keep_alive, Some(2222)); // JSON wins (CLI absent)
    }

    #[test]
    fn json_config_roundtrips_serialization() {
        let json = r#"{
            "secret": "abc",
            "server": "127.0.0.1:5555",
            "transport": {
                "tls_mode": "tls",
                "congestion": "Bbr",
                "max_streams": 50
            }
        }"#;
        let parsed = json::JsonConfig::from_json_str(json).unwrap();
        let reserialized = serde_json::to_string(&parsed).unwrap();
        let reparsed = json::JsonConfig::from_json_str(&reserialized).unwrap();
        assert_eq!(reparsed.secret.as_deref(), Some("abc"));
        assert_eq!(reparsed.server.as_deref(), Some("127.0.0.1:5555"));
    }

    #[test]
    fn tls_mode_serializes_kebab_case() {
        let s_tls = serde_json::to_string(&TlsMode::Tls).unwrap();
        let s_mtls = serde_json::to_string(&TlsMode::MTls).unwrap();
        let s_insecure = serde_json::to_string(&TlsMode::Insecure).unwrap();
        assert_eq!(s_tls, "\"tls\"");
        assert_eq!(s_mtls, "\"m-tls\"");
        assert_eq!(s_insecure, "\"insecure\"");
    }

    #[test]
    fn tls_mode_default_is_tls() {
        assert_eq!(TlsMode::default(), TlsMode::Tls);
    }

    #[test]
    fn endpoint_config_defaults_are_empty() {
        let cfg = EndpointConfig::default();
        #[cfg(feature = "endpoint-socks")]
        assert!(cfg.socks.is_none());
        #[cfg(feature = "endpoint-http")]
        assert!(cfg.http.is_none());
        let _ = cfg;
    }

    #[test]
    fn load_from_file_missing_path_returns_error() {
        let path = std::path::Path::new("/this/path/does/not/exist/config.json");
        let result = load_from_file(path);
        assert!(result.is_err());
    }

    #[test]
    fn load_from_file_reads_real_file() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("ombrac-client-cfg-{}.json", std::process::id()));
        std::fs::write(
            &path,
            r#"{"secret":"k","server":"1.1.1.1:443"}"#,
        )
        .unwrap();

        let cfg = load_from_file(&path).unwrap();
        assert_eq!(cfg.secret, "k");
        assert_eq!(cfg.server, "1.1.1.1:443");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn config_builder_build_fails_without_required_fields() {
        let err = ConfigBuilder::new().build().unwrap_err();
        assert!(err.contains("secret"));
    }

    #[cfg(feature = "endpoint-socks")]
    #[test]
    fn endpoint_socks_address_parses_from_json() {
        let json = r#"{
            "secret": "k",
            "server": "s:1",
            "endpoint": { "socks": "127.0.0.1:1080" }
        }"#;
        let cfg = load_from_json(json).unwrap();
        assert_eq!(
            cfg.endpoint.socks.unwrap().to_string(),
            "127.0.0.1:1080"
        );
    }
}
