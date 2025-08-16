pub mod address;
#[cfg(feature = "datagram")]
pub mod associate;
pub mod client;
pub mod connect;
pub mod io;
pub mod server;

const SECRET_LENGTH: usize = 32;

pub type Secret = [u8; SECRET_LENGTH];

pub mod prelude {
    pub use super::Secret;
    pub use super::address::Address;
    #[cfg(feature = "datagram")]
    pub use super::associate::Associate;
    pub use super::client::{Client, Stream};
    pub use super::connect::Connect;
    pub use super::server::Server;
}
