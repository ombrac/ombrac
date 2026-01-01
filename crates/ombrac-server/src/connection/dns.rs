use std::io;
use std::net::SocketAddr;

use hickory_resolver::TokioResolver;
use tokio::sync::OnceCell;

// Global DNS resolver instance using hickory-resolver
static DNS_RESOLVER: OnceCell<TokioResolver> = OnceCell::const_new();

/// Gets or initializes the global DNS resolver instance.
///
/// The resolver is created from system configuration on first use.
pub(crate) async fn get_dns_resolver() -> &'static TokioResolver {
    DNS_RESOLVER
        .get_or_init(|| async {
            TokioResolver::builder_tokio()
                .expect("failed to create dns resolver from system config")
                .build()
        })
        .await
}

/// Resolves a domain name to a socket address using hickory-resolver.
///
/// This function performs DNS resolution for domain names. For IP addresses,
/// it returns them directly without DNS lookup.
///
/// # Arguments
///
/// * `domain` - The domain name as bytes
/// * `port` - The port number
///
/// # Errors
///
/// Returns an error if DNS resolution fails or no IP addresses are found.
pub(crate) async fn resolve_domain(domain: &[u8], port: u16) -> io::Result<SocketAddr> {
    let domain_str = std::str::from_utf8(domain).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "domain name contains invalid utf-8 characters",
        )
    })?;

    // Use hickory-resolver for DNS resolution
    let resolver = get_dns_resolver().await;
    let lookup_result = resolver.lookup_ip(domain_str).await.map_err(|e| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("dns resolution failed for {}: {}", domain_str, e),
        )
    })?;

    // Get the first IP address (hickory-resolver returns addresses in preferred order)
    lookup_result
        .iter()
        .next()
        .map(|ip| SocketAddr::new(ip, port))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("no ip addresses found for {}", domain_str),
            )
        })
}
