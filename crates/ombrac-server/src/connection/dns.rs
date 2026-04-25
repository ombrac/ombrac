use std::io;
use std::net::SocketAddr;

use hickory_resolver::TokioResolver;
use tokio::sync::OnceCell;

// Global DNS resolver instance using hickory-resolver
static DNS_RESOLVER: OnceCell<TokioResolver> = OnceCell::const_new();

/// Gets or initializes the global DNS resolver instance.
///
/// The resolver is created from system configuration on first use.
///
/// # Errors
///
/// Returns an error if the system DNS configuration cannot be read.
pub(crate) async fn get_dns_resolver() -> io::Result<&'static TokioResolver> {
    DNS_RESOLVER
        .get_or_try_init(|| async {
            TokioResolver::builder_tokio()
                .map(|b| b.build())
                .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("failed to create dns resolver from system config: {e}")))
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
    let resolver = get_dns_resolver().await?;
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── Group I: resolve_domain() ────────────────────────────────────────────

    #[tokio::test]
    async fn test_resolve_domain_valid_localhost() {
        let result = resolve_domain(b"localhost", 80).await;
        let addr = result.expect("localhost should resolve");
        assert_eq!(addr.port(), 80);
    }

    #[tokio::test]
    async fn test_resolve_domain_ipv4_literal() {
        let result = resolve_domain(b"127.0.0.1", 9000).await;
        let addr = result.expect("IP literal should resolve");
        assert_eq!(addr.port(), 9000);
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
    }

    #[tokio::test]
    async fn test_resolve_domain_invalid_utf8() {
        let err = resolve_domain(b"\xff\xfe", 80)
            .await
            .expect_err("invalid utf-8 should fail");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("utf-8"));
    }

    #[tokio::test]
    async fn test_resolve_domain_empty_bytes() {
        let result = resolve_domain(b"", 80).await;
        assert!(result.is_err(), "empty domain should fail");
    }

    #[tokio::test]
    async fn test_resolve_domain_nonexistent() {
        // .invalid TLD is guaranteed non-resolvable per RFC 2606
        let err = resolve_domain(
            b"this-absolutely-does-not-exist.ombrac-test-invalid",
            80,
        )
        .await
        .expect_err("non-existent domain should fail");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
