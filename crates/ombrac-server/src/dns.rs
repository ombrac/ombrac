use std::io;
use std::net::IpAddr;
use std::sync::LazyLock;

use hickory_resolver::config::{ResolverConfig, ResolverOpts};
use hickory_resolver::name_server::{GenericConnector, TokioRuntimeProvider};
use hickory_resolver::{AsyncResolver, TokioAsyncResolver};

static RESOLVER: LazyLock<AsyncResolver<GenericConnector<TokioRuntimeProvider>>> =
    LazyLock::new(|| {
        let config = ResolverConfig::google_h3();
        let options = ResolverOpts::default();
        
        TokioAsyncResolver::tokio(config, options)
    });

pub(crate) async fn lookup_ip(host: &str) -> io::Result<IpAddr> {
    let response = RESOLVER
        .lookup_ip(host)
        .await
        .map_err(|e| io::Error::other(e.to_string()))?;

    response.iter().next().ok_or(io::Error::other(format!(
        "failed to resolve domain {}",
        host
    )))
}
