mod server;

use std::net::SocketAddr;

pub use server::SocksServer;

// SOCKS Server Config
pub struct Config {
    listen: SocketAddr,
}

impl Config {
    pub fn new(listen: SocketAddr) -> Self {
        Self { listen }
    }
}
