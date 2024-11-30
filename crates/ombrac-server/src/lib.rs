mod macros;

pub mod transport;

pub struct Server<T> {
    inner: T,
}
