mod connection;
mod server;
mod stream;

use std::net::SocketAddr;
use std::time::Duration;

pub use server::impl_s2n_quic::NoiseQuic;

pub struct Config {
    listen: SocketAddr,

    tls_key: String,
    tls_cert: String,

    initial_congestion_window: Option<u32>,

    max_handshake_duration: Option<Duration>,
    max_idle_timeout: Option<Duration>,
    max_keep_alive_period: Option<Duration>,
    max_open_bidirectional_streams: Option<u64>,

    bidirectional_local_data_window: Option<u64>,
    bidirectional_remote_data_window: Option<u64>,
}

impl Config {
    pub fn new<T>(listen: T, tls_cert: String, tls_key: String) -> Self
    where
        T: Into<SocketAddr>,
    {
        Config {
            listen: listen.into(),
            tls_cert,
            tls_key,
            initial_congestion_window: None,
            max_handshake_duration: None,
            max_idle_timeout: None,
            max_keep_alive_period: None,
            max_open_bidirectional_streams: None,
            bidirectional_local_data_window: None,
            bidirectional_remote_data_window: None,
        }
    }

    pub fn with_tls_cert(mut self, tls_cert: String) -> Self {
        self.tls_cert = tls_cert;
        self
    }

    pub fn with_tls_key(mut self, tls_key: String) -> Self {
        self.tls_key = tls_key;
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
