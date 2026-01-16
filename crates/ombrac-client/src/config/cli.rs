use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;
use clap::builder::Styles;
use clap::builder::styling::{AnsiColor, Style};

use ombrac_transport::quic::Congestion;

use crate::config::{EndpointConfig, TlsMode, TransportConfig};

/// Command-line arguments for the ombrac client
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

    /// Extended parameter of the protocol, used for authentication related information
    #[clap(
        long,
        help_heading = "Protocol",
        value_name = "STR",
        alias = "handshake-option"
    )]
    pub auth_option: Option<String>,

    #[clap(flatten)]
    pub endpoint: CliEndpointConfig,

    #[clap(flatten)]
    pub transport: CliTransportConfig,

    #[cfg(feature = "tracing")]
    #[clap(flatten)]
    pub logging: CliLoggingConfig,
}

/// CLI-specific endpoint configuration
#[derive(Parser, Debug, Clone, Default)]
pub struct CliEndpointConfig {
    /// The address to bind for the HTTP/HTTPS server
    #[cfg(feature = "endpoint-http")]
    #[clap(long, value_name = "ADDR", help_heading = "Endpoint")]
    pub http: Option<SocketAddr>,

    /// The address to bind for the SOCKS server
    #[cfg(feature = "endpoint-socks")]
    #[clap(long, value_name = "ADDR", help_heading = "Endpoint")]
    pub socks: Option<SocketAddr>,

    #[cfg(feature = "endpoint-tun")]
    #[clap(flatten)]
    pub tun: Option<CliTunConfig>,
}

impl CliEndpointConfig {
    /// Convert CLI endpoint config to internal EndpointConfig
    pub fn into_endpoint_config(self) -> EndpointConfig {
        EndpointConfig {
            #[cfg(feature = "endpoint-http")]
            http: self.http,
            #[cfg(feature = "endpoint-socks")]
            socks: self.socks,
            #[cfg(feature = "endpoint-tun")]
            tun: self.tun.map(|t| t.into_tun_config()),
        }
    }
}

/// CLI-specific TUN configuration
#[cfg(feature = "endpoint-tun")]
#[derive(Parser, Debug, Clone, Default)]
pub struct CliTunConfig {
    /// Use a pre-existing TUN device by providing its file descriptor
    /// `tun_ipv4`, `tun_ipv6`, and `tun_mtu` will be ignored
    #[clap(long, help_heading = "Endpoint", value_name = "FD")]
    pub tun_fd: Option<i32>,

    /// The IPv4 address and subnet for the TUN device, in CIDR notation
    #[clap(long, help_heading = "Endpoint", value_name = "CIDR")]
    pub tun_ipv4: Option<String>,

    /// The IPv6 address and subnet for the TUN device, in CIDR notation
    #[clap(long, help_heading = "Endpoint", value_name = "CIDR")]
    pub tun_ipv6: Option<String>,

    /// The Maximum Transmission Unit (MTU) for the TUN device. [default: 1280]
    ///
    /// Defaults to 1280 bytes (IPv6 minimum MTU) for maximum compatibility and to avoid
    /// fragmentation issues across diverse network paths, especially important for VPN tunnels.
    #[clap(long, help_heading = "Endpoint", value_name = "MTU")]
    pub tun_mtu: Option<u16>,

    /// The IPv4 address pool for the built-in fake DNS server, in CIDR notation. [default: 198.18.0.0/16]
    #[clap(long, help_heading = "Endpoint", value_name = "CIDR")]
    pub fake_dns: Option<String>,

    /// Disable UDP traffic to port 443
    #[clap(long, help_heading = "Endpoint", value_name = "BOOL")]
    pub disable_udp_443: Option<bool>,
}

#[cfg(feature = "endpoint-tun")]
impl CliTunConfig {
    /// Convert CLI TUN config to internal TunConfig
    pub fn into_tun_config(self) -> crate::config::TunConfig {
        crate::config::TunConfig {
            tun_fd: self.tun_fd,
            tun_ipv4: self.tun_ipv4,
            tun_ipv6: self.tun_ipv6,
            tun_mtu: self.tun_mtu,
            fake_dns: self.fake_dns,
            disable_udp_443: self.disable_udp_443,
        }
    }
}

/// CLI-specific transport configuration
#[derive(Parser, Debug, Clone)]
pub struct CliTransportConfig {
    /// The address to bind for transport
    #[clap(long, help_heading = "Transport", value_name = "ADDR")]
    pub bind: Option<SocketAddr>,

    /// Name of the server to connect (derived from `server` if not provided)
    #[clap(long, help_heading = "Transport", value_name = "STR")]
    pub server_name: Option<String>,

    /// Set the TLS mode for the connection
    #[clap(long, value_enum, help_heading = "Transport")]
    pub tls_mode: Option<TlsMode>,

    /// Path to the Certificate Authority (CA) certificate file
    /// in 'TLS' mode, if not provided, the system's default root certificates are used
    #[clap(long, help_heading = "Transport", value_name = "FILE")]
    pub ca_cert: Option<PathBuf>,

    /// Path to the client's TLS certificate for mTLS
    #[clap(long, help_heading = "Transport", value_name = "FILE")]
    pub client_cert: Option<PathBuf>,

    /// Path to the client's TLS private key for mTLS
    #[clap(long, help_heading = "Transport", value_name = "FILE")]
    pub client_key: Option<PathBuf>,

    /// Enable 0-RTT for faster connection establishment
    #[clap(long, help_heading = "Transport", value_name = "BOOL")]
    pub zero_rtt: Option<bool>,

    /// Application-Layer protocol negotiation (ALPN) protocols [default: h3]
    #[clap(
        long,
        help_heading = "Transport",
        value_name = "PROTOCOLS",
        value_delimiter = ','
    )]
    pub alpn_protocols: Option<Vec<Vec<u8>>>,

    /// Congestion control algorithm to use (e.g. bbr, cubic, newreno) [default: bbr]
    #[clap(long, help_heading = "Transport", value_name = "ALGORITHM")]
    pub congestion: Option<Congestion>,

    /// Initial congestion window size in bytes
    #[clap(long, help_heading = "Transport", value_name = "NUM")]
    pub cwnd_init: Option<u64>,

    /// Maximum idle time (in milliseconds) before closing the connection [default: 30000]
    #[clap(long, help_heading = "Transport", value_name = "TIME")]
    pub idle_timeout: Option<u64>,

    /// Keep-alive interval (in milliseconds) [default: 8000]
    #[clap(long, help_heading = "Transport", value_name = "TIME")]
    pub keep_alive: Option<u64>,

    /// Maximum number of bidirectional streams that can be open simultaneously [default: 100]
    #[clap(long, help_heading = "Transport", value_name = "NUM")]
    pub max_streams: Option<u64>,
}

/// CLI-specific logging configuration
#[cfg(feature = "tracing")]
#[derive(Parser, Debug, Clone)]
pub struct CliLoggingConfig {
    /// Logging level (e.g., INFO, WARN, ERROR) [default: INFO]
    #[clap(long, help_heading = "Logging", value_name = "LEVEL")]
    pub log_level: Option<String>,
}

impl CliTransportConfig {
    /// Convert CLI transport config to internal TransportConfig
    pub fn into_transport_config(self) -> TransportConfig {
        TransportConfig {
            bind: self.bind,
            server_name: self.server_name,
            tls_mode: self.tls_mode,
            ca_cert: self.ca_cert,
            client_cert: self.client_cert,
            client_key: self.client_key,
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
    pub server: Option<String>,
    pub auth_option: Option<String>,
    pub endpoint: EndpointConfig,
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
            server: args.server,
            auth_option: args.auth_option,
            endpoint: args.endpoint.into_endpoint_config(),
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
