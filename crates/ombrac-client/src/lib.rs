mod macros;

pub mod endpoint;
pub mod transport;

pub struct Client<T> {
    inner: T,
}
