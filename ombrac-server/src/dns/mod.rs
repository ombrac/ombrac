use std::io::{Error, Result};
use std::net::SocketAddr;

use hickory_resolver::config::{ResolverConfig, ResolverOpts};
use hickory_resolver::TokioAsyncResolver;
use ombrac_protocol::Resolver as OmbracResolver;

pub struct Resolver(TokioAsyncResolver);

impl From<(ResolverConfig, ResolverOpts)> for Resolver {
    fn from(value: (ResolverConfig, ResolverOpts)) -> Self {
        Self(TokioAsyncResolver::tokio(value.0, value.1))
    }
}

impl OmbracResolver for Resolver {
    async fn lookup(&self, domain: &str, port: u16) -> Result<SocketAddr> {
        let response = self.0.lookup_ip(domain).await?;
        let address = response
            .iter()
            .next()
            .ok_or_else(|| Error::other(format!("could not resolve domain '{}'", domain)))?;

        Ok(SocketAddr::new(address, port))
    }
}
