use std::io;
use std::net::SocketAddr;

pub(crate) async fn lookup_ip(host: &str) -> io::Result<SocketAddr> {
    use tokio::net::lookup_host;

    let response = lookup_host(host)
        .await
        .map_err(|e| io::Error::other(e.to_string()))?;

    response.into_iter().next().ok_or(io::Error::other(format!(
        "failed to resolve domain {}",
        host
    )))
}
