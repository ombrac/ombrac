mod client;

use std::error::Error;
use std::io;
use std::net::SocketAddr;
use std::time::Duration;

pub struct Config {
    bind: Option<String>,

    server_name: Option<String>,
    server_address: String,

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
    pub fn new(server_address: String) -> Self {
        Config {
            bind: None,
            server_name: None,
            server_address,
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

    pub fn with_server_name(mut self, server_name: String) -> Self {
        self.server_name = Some(server_name);
        self
    }

    pub fn with_bind(mut self, bind: String) -> Self {
        self.bind = Some(bind);
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

    fn server_name(&self) -> io::Result<&str> {
        match &self.server_name {
            Some(value) => Ok(value),
            None => {
                let pos = self
                    .server_address
                    .rfind(':')
                    .ok_or(io::Error::other(format!(
                        "invalid address {}",
                        self.server_address
                    )))?;

                Ok(&self.server_address[..pos])
            }
        }
    }

    async fn server_socket_address(&self) -> io::Result<SocketAddr> {
        use tokio::net::lookup_host;

        lookup_host(&self.server_address)
            .await?
            .next()
            .ok_or(io::Error::other(format!(
                "unable to resolve address '{}'",
                self.server_address
            )))
    }
}

mod impl_s2n_quic {
    use super::*;

    use super::client::impl_s2n_quic::*;

    impl crate::Client<NoiseClient> {
        pub async fn new(config: Config) -> Result<Self, Box<dyn Error>> {
            Ok(Self {
                inner: NoiseClient::new(config).await?,
            })
        }
    }

    impl ombrac::Client<Stream> for crate::Client<NoiseClient> {
        async fn outbound(&mut self) -> Option<Stream> {
            self.inner.stream().await
        }
    }
}
