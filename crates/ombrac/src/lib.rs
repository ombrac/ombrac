pub mod address;
pub mod associate;
pub mod connect;
pub mod io;

const SECRET_LENGTH: usize = 32;

pub type Secret = [u8; SECRET_LENGTH];

pub mod prelude {
    pub use super::Secret;
    pub use super::address::Address;
    pub use super::associate::Associate;
    pub use super::connect::Connect;
}
