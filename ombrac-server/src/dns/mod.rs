use std::io::{Error, Result};
use std::net::SocketAddr;

use hickory_resolver::TokioAsyncResolver;
use ombrac_protocol::Resolver as OmbracResolver;

pub struct Resolver(TokioAsyncResolver);

impl Default for Resolver {
    fn default() -> Self {
        use hickory_resolver::config::{ResolverConfig, ResolverOpts};

        Self(TokioAsyncResolver::tokio(
            ResolverConfig::default(),
            ResolverOpts::default(),
        ))
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
