mod client;
mod connection;
mod stream;

use std::error::Error;
use std::net::SocketAddr;
use std::time::Duration;

pub use client::impl_s2n_quic::NoiseQuic;

// QUIC Config
pub struct Config {
    bind: SocketAddr,

    server_name: String,
    server_address: SocketAddr,

    tls_cert: Option<String>,

    initial_congestion_window: Option<u32>,

    max_handshake_duration: Option<Duration>,
    max_idle_timeout: Option<Duration>,
    max_keep_alive_period: Option<Duration>,
    max_open_bidirectional_streams: Option<u64>,

    bidirectional_local_data_window: Option<u64>,
    bidirectional_remote_data_window: Option<u64>,
}

impl Config {
    pub fn new<T>(bind: T, name: String, address: T) -> Self
    where
        T: Into<SocketAddr>,
    {
        Config {
            bind: bind.into(),
            server_name: name,
            server_address: address.into(),
            tls_cert: None,
            initial_congestion_window: None,
            max_handshake_duration: None,
            max_idle_timeout: None,
            max_keep_alive_period: None,
            max_open_bidirectional_streams: None,
            bidirectional_local_data_window: None,
            bidirectional_remote_data_window: None,
        }
    }

    pub fn with_address(address: String) -> Result<Self, Box<dyn Error>> {
        use std::net::ToSocketAddrs;

        let pos = address.rfind(':').ok_or("invalid remote address")?;
        let server_name = String::from(&address[..pos]);

        let server_address = address
            .to_socket_addrs()?
            .nth(0)
            .ok_or(format!("unable to resolve address {}", server_name))?;

        let bind = {
            let address = match server_address {
                SocketAddr::V4(_) => "0.0.0.0:0",
                SocketAddr::V6(_) => "[::]:0",
            };
            address.parse().expect("failed to parse socket address")
        };

        Ok(Self::new(bind, server_name, server_address))
    }

    pub fn with_server_name(mut self, server_name: String) -> Self {
        self.server_name = server_name;
        self
    }

    pub fn with_bind(mut self, bind: SocketAddr) -> Self {
        self.bind = bind;
        self
    }

    pub fn with_tls_cert(mut self, tls_cert: String) -> Self {
        self.tls_cert = Some(tls_cert);
        self
    }

    pub fn with_initial_congestion_window(mut self, window: u32) -> Self {
        self.initial_congestion_window = Some(window);
        self
    }

    pub fn with_max_handshake_duration(mut self, duration: Duration) -> Self {
        self.max_handshake_duration = Some(duration);
        self
    }

    pub fn with_max_idle_timeout(mut self, duration: Duration) -> Self {
        self.max_idle_timeout = Some(duration);
        self
    }

    pub fn with_max_keep_alive_period(mut self, duration: Duration) -> Self {
        self.max_keep_alive_period = Some(duration);
        self
    }

    pub fn with_max_open_bidirectional_streams(mut self, streams: u64) -> Self {
        self.max_open_bidirectional_streams = Some(streams);
        self
    }

    pub fn with_bidirectional_local_data_window(mut self, window: u64) -> Self {
        self.bidirectional_local_data_window = Some(window);
        self
    }

    pub fn with_bidirectional_remote_data_window(mut self, window: u64) -> Self {
        self.bidirectional_remote_data_window = Some(window);
        self
    }
}
