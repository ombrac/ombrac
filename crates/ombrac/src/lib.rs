pub mod address;
pub mod connect;
pub mod io;
pub mod packet;

const SECRET_LENGTH: usize = 32;

pub type Secret = [u8; SECRET_LENGTH];

pub mod prelude {
    pub use super::address::Address;
    pub use super::connect::Connect;
    pub use super::packet::Packet;
    pub use super::Secret;
}
