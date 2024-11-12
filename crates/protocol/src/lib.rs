pub mod client;
pub mod request;
pub mod server;

mod io;

use std::future::Future;
use std::io::Result;
use std::net::SocketAddr;

pub trait Provider<T> {
    fn fetch(&mut self) -> impl Future<Output = Option<T>> + Send;
}

pub trait Resolver {
    fn lookup(&self, domain: &str, port: u16) -> impl Future<Output = Result<SocketAddr>> + Send;
}
