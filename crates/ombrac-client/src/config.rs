use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;

use clap::builder::Styles;
use clap::builder::styling::{AnsiColor, Style};
use clap::{Parser, ValueEnum};
use figment::Figment;
use figment::providers::{Format, Json, Serialized};
use serde::{Deserialize, Serialize};

#[cfg(feature = "transport-quic")]
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

    /// Address of the server to connect to
    #[clap(
        long,
        short = 's',
        help_heading = "Required",
        value_name = "ADDR",
        required_unless_present = "config"
    )]
    pub server: Option<String>,

    #[clap(flatten)]
    pub endpoint: EndpointConfig,

    #[cfg(feature = "transport-quic")]
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
    pub server: Option<String>,

    pub endpoint: EndpointConfig,

    #[cfg(feature = "transport-quic")]
    pub transport: TransportConfig,

    #[cfg(feature = "tracing")]
    pub logging: LoggingConfig,
}

#[derive(Deserialize, Serialize, Debug, Parser, Clone, Default)]
pub struct EndpointConfig {
    /// The address to bind for the HTTP/HTTPS server
    #[cfg(feature = "endpoint-http")]
    #[clap(long, value_name = "ADDR", help_heading = "Endpoint")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http: Option<SocketAddr>,

    /// The address to bind for the SOCKS server
    #[cfg(feature = "endpoint-socks")]
    #[clap(long, value_name = "ADDR", help_heading = "Endpoint")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub socks: Option<SocketAddr>,

    #[cfg(feature = "endpoint-tun")]
    #[clap(flatten)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tun: Option<TunConfig>,
}

#[derive(Deserialize, Serialize, Debug, Parser, Clone)]
#[cfg(feature = "transport-quic")]
pub struct TransportConfig {
    /// The address to bind for transport
    #[clap(long, help_heading = "Transport", value_name = "ADDR")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bind: Option<SocketAddr>,

    /// Name of the server to connect (derived from `server` if not provided)
    #[clap(long, help_heading = "Transport", value_name = "STR")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_name: Option<String>,

    /// Set the TLS mode for the connection
    /// tls: Standard TLS with server certificate verification
    /// m-tls: Mutual TLS with client and server certificate verification
    /// insecure: Skip server certificate verification (for testing only)
    #[clap(long, value_enum, help_heading = "Transport")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_mode: Option<TlsMode>,

    /// Path to the Certificate Authority (CA) certificate file
    /// in 'TLS' mode, if not provided, the system's default root certificates are used
    #[clap(long, help_heading = "Transport", value_name = "FILE")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ca_cert: Option<PathBuf>,

    /// Path to the client's TLS certificate for mTLS
    #[clap(long, help_heading = "Transport", value_name = "FILE")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_cert: Option<PathBuf>,

    /// Path to the client's TLS private key for mTLS
    #[clap(long, help_heading = "Transport", value_name = "FILE")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_key: Option<PathBuf>,

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
    /// 30 second default recommended by RFC 9308
    #[clap(long, help_heading = "Transport", value_name = "TIME")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idle_timeout: Option<u64>,

    /// Keep-alive interval (in milliseconds) [default: 8000]
    #[clap(long, help_heading = "Transport", value_name = "TIME")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<u64>,

    /// Maximum number of bidirectional streams that can be open simultaneously [default: 100]
    #[clap(long, help_heading = "Transport", value_name = "NUM")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_streams: Option<u64>,
}

#[cfg(feature = "tracing")]
#[derive(Deserialize, Serialize, Debug, Parser, Clone)]
pub struct LoggingConfig {
    /// Logging level (e.g., INFO, WARN, ERROR) [default: INFO]
    #[clap(long, help_heading = "Logging", value_name = "LEVEL")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_level: Option<String>,
}

#[cfg(feature = "endpoint-tun")]
#[derive(Deserialize, Serialize, Debug, Parser, Clone)]
pub struct TunConfig {
    /// Use a pre-existing TUN device by providing its file descriptor.  
    /// `tun_ipv4`, `tun_ipv6`, and `tun_mtu` will be ignored.
    #[clap(long, help_heading = "Endpoint", value_name = "FD")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tun_fd: Option<i32>,

    /// The IPv4 address and subnet for the TUN device, in CIDR notation (e.g., 198.19.0.1/24).
    #[clap(long, help_heading = "Endpoint", value_name = "CIDR")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tun_ipv4: Option<String>,

    /// The IPv6 address and subnet for the TUN device, in CIDR notation (e.g., fd00::1/64).
    #[clap(long, help_heading = "Endpoint", value_name = "CIDR")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tun_ipv6: Option<String>,

    /// The Maximum Transmission Unit (MTU) for the TUN device. [default: 1500]
    #[clap(long, help_heading = "Endpoint", value_name = "U16")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tun_mtu: Option<u16>,

    /// The IPv4 address pool for the built-in fake DNS server, in CIDR notation. [default: 198.18.0.0/16]
    #[clap(long, help_heading = "Endpoint", value_name = "CIDR")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fake_dns: Option<String>,
}

#[cfg(feature = "transport-quic")]
#[derive(ValueEnum, Clone, Debug, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum TlsMode {
    #[default]
    Tls,
    MTls,
    Insecure,
}

#[derive(Debug, Clone)]
pub struct ServiceConfig {
    pub secret: String,
    pub server: String,
    pub endpoint: EndpointConfig,
    #[cfg(feature = "transport-quic")]
    pub transport: TransportConfig,
    #[cfg(feature = "tracing")]
    pub logging: LoggingConfig,
}

#[cfg(feature = "transport-quic")]
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

#[cfg(feature = "transport-quic")]
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

#[cfg(feature = "tracing")]
impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            log_level: Some("INFO".to_string()),
        }
    }
}

pub fn load() -> Result<ServiceConfig, Box<figment::Error>> {
    let args = Args::parse();

    let mut figment = Figment::new().merge(Serialized::defaults(ConfigFile::default()));

    if let Some(config_path) = &args.config {
        if !config_path.exists() {
            let err = io::Error::new(
                io::ErrorKind::NotFound,
                format!("Configuration file not found: {}", config_path.display()),
            );
            return Err(Box::new(figment::Error::from(err.to_string())));
        }

        figment = figment.merge(Json::file(config_path));
    }

    let cli_overrides = ConfigFile {
        secret: args.secret,
        server: args.server,
        endpoint: args.endpoint,
        #[cfg(feature = "transport-quic")]
        transport: args.transport,
        #[cfg(feature = "tracing")]
        logging: args.logging,
    };

    figment = figment.merge(Serialized::defaults(cli_overrides));

    let config: ConfigFile = figment.extract()?;

    let secret = config
        .secret
        .ok_or_else(|| figment::Error::from("missing field `secret`"))?;
    let server = config
        .server
        .ok_or_else(|| figment::Error::from("missing field `server`"))?;

    Ok(ServiceConfig {
        secret,
        server,
        endpoint: config.endpoint,
        #[cfg(feature = "transport-quic")]
        transport: config.transport,
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
