//! Integration tests for `TlsMode::Tls` and `TlsMode::MTls`.
//!
//! The existing tests use `enable_self_signed = true` + `skip_server_verification = true`
//! (the Insecure path). These tests exercise the *real* TLS verification paths:
//!
//! - TLS: client trusts a custom CA, server uses a cert signed by that CA.
//! - mTLS: same as TLS, plus the server requires the client to present a cert
//!   signed by a CA it trusts.
//!
//! Bonus: a negative test that verifies untrusted certs are rejected.

use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::broadcast;

use ombrac::protocol::{Address, Secret};
use ombrac_client::client::Client as TunnelClient;
use ombrac_server::connection::ConnectionAcceptor;
use ombrac_transport::quic::Connection as QuicConnection;
use ombrac_transport::quic::client::{Client as QuicClient, Config as QuicClientCfg};
use ombrac_transport::quic::server::{Config as QuicServerCfg, Server as QuicServer};

fn random_secret() -> Secret {
    use rand::Rng;
    let mut s = [0u8; 32];
    let mut rng = rand::rng();
    rng.fill_bytes(&mut s);
    s
}

/// Owned PKI on disk; files are cleaned up on drop.
struct PkiPaths {
    dir: PathBuf,
    ca_cert: PathBuf,
    server_cert: PathBuf,
    server_key: PathBuf,
    client_cert: PathBuf,
    client_key: PathBuf,
}

impl Drop for PkiPaths {
    fn drop(&mut self) {
        // Best-effort cleanup. Failures here don't matter for test correctness.
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

static PKI_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Generates a fresh CA + server cert (for localhost / 127.0.0.1) + client cert,
/// all written to a temporary directory. Returns absolute paths suitable for
/// passing into `QuicClientCfg::root_ca_path` / `QuicServerCfg::tls_cert_key_paths`.
fn generate_pki() -> PkiPaths {
    use rcgen::string::Ia5String;
    use rcgen::{
        BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair, KeyUsagePurpose, SanType,
    };
    use std::str::FromStr;

    let unique = PKI_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "ombrac-tls-test-{}-{}",
        std::process::id(),
        unique
    ));
    std::fs::create_dir_all(&dir).expect("create test pki dir");

    // 1. Build a root CA and turn it into an Issuer for signing children.
    let mut ca_params = CertificateParams::default();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "ombrac-test-ca");
    let ca_key = KeyPair::generate().unwrap();
    let ca_cert = ca_params.self_signed(&ca_key).unwrap();
    let ca_cert_path = dir.join("ca.pem");
    std::fs::write(&ca_cert_path, ca_cert.pem()).unwrap();

    let ca_issuer = Issuer::new(ca_params, ca_key);

    // 2. Server cert (SAN: localhost + 127.0.0.1), signed by CA.
    let mut server_params = CertificateParams::default();
    server_params.subject_alt_names = vec![
        SanType::DnsName(Ia5String::from_str("localhost").unwrap()),
        SanType::IpAddress(std::net::IpAddr::from([127, 0, 0, 1])),
    ];
    server_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "ombrac-test-server");
    let server_key = KeyPair::generate().unwrap();
    let server_cert = server_params.signed_by(&server_key, &ca_issuer).unwrap();

    let server_cert_path = dir.join("server.pem");
    let server_key_path = dir.join("server.key.pem");
    std::fs::write(&server_cert_path, server_cert.pem()).unwrap();
    std::fs::write(&server_key_path, server_key.serialize_pem()).unwrap();

    // 3. Client cert, signed by the same CA.
    let mut client_params = CertificateParams::default();
    client_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "ombrac-test-client");
    let client_key = KeyPair::generate().unwrap();
    let client_cert = client_params.signed_by(&client_key, &ca_issuer).unwrap();

    let client_cert_path = dir.join("client.pem");
    let client_key_path = dir.join("client.key.pem");
    std::fs::write(&client_cert_path, client_cert.pem()).unwrap();
    std::fs::write(&client_key_path, client_key.serialize_pem()).unwrap();

    PkiPaths {
        dir,
        ca_cert: ca_cert_path,
        server_cert: server_cert_path,
        server_key: server_key_path,
        client_cert: client_cert_path,
        client_key: client_key_path,
    }
}

async fn spawn_tcp_echo() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let (mut r, mut w) = stream.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    addr
}

/// Spawns a real QUIC + ombrac server using the given PKI material. Server
/// presents `server_cert/server_key`; if `verify_client_ca` is `Some`, mTLS
/// is enabled and the server verifies the client against that CA.
async fn spawn_server(
    pki: &PkiPaths,
    verify_client_ca: Option<&PathBuf>,
) -> (SocketAddr, Secret, broadcast::Sender<()>) {
    let server_udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let server_addr = server_udp.local_addr().unwrap();
    let secret = random_secret();

    let mut server_cfg = QuicServerCfg::default();
    server_cfg.tls_cert_key_paths = Some((pki.server_cert.clone(), pki.server_key.clone()));
    server_cfg.alpn_protocols = vec![b"h3".to_vec()];
    if let Some(ca) = verify_client_ca {
        server_cfg.root_ca_path = Some(ca.clone());
    }

    let quic_server = QuicServer::new(server_udp, server_cfg).await.unwrap();
    let (shutdown_tx, shutdown_rx) = broadcast::channel::<()>(1);
    tokio::spawn(async move {
        let acceptor = ConnectionAcceptor::new(quic_server, secret);
        let _ = acceptor.accept_loop(shutdown_rx).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    (server_addr, secret, shutdown_tx)
}

fn build_quic_client_cfg(
    server_addr: SocketAddr,
    root_ca: Option<PathBuf>,
    client_cert_key: Option<(PathBuf, PathBuf)>,
) -> QuicClientCfg {
    let mut cfg = QuicClientCfg::new(server_addr, "localhost".to_string());
    cfg.alpn_protocols = vec![b"h3".to_vec()];
    cfg.root_ca_path = root_ca;
    cfg.client_cert_key_paths = client_cert_key;
    cfg
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
#[ntest::timeout(60000)]
async fn tls_mode_client_trusts_ca_succeeds() -> io::Result<()> {
    let pki = generate_pki();
    let (server_addr, secret, shutdown) = spawn_server(&pki, None).await;

    let echo = spawn_tcp_echo().await;
    let client_cfg = build_quic_client_cfg(server_addr, Some(pki.ca_cert.clone()), None);
    let quic_client = QuicClient::new(client_cfg).unwrap();

    let tunnel: TunnelClient<QuicClient, QuicConnection> =
        TunnelClient::new(quic_client, secret, None).await.unwrap();

    let dest: Address = echo.to_string().as_str().try_into().unwrap();
    let mut stream = tunnel.open_bidirectional(dest).await?;
    stream.write_all(b"tls-mode").await?;
    stream.flush().await?;
    let mut buf = [0u8; 8];
    stream.read_exact(&mut buf).await?;
    assert_eq!(&buf, b"tls-mode");

    let _ = shutdown.send(());
    Ok(())
}

#[tokio::test]
#[ntest::timeout(60000)]
async fn tls_mode_untrusted_cert_rejected() {
    let server_pki = generate_pki();
    let unrelated_pki = generate_pki(); // a different CA
    let (server_addr, secret, shutdown) = spawn_server(&server_pki, None).await;

    // Client trusts the wrong CA — verification must fail.
    let client_cfg = build_quic_client_cfg(server_addr, Some(unrelated_pki.ca_cert.clone()), None);
    let quic_client = QuicClient::new(client_cfg).unwrap();

    let result = TunnelClient::new(quic_client, secret, None).await;
    assert!(
        result.is_err(),
        "client should reject server signed by untrusted CA"
    );

    let _ = shutdown.send(());
}

#[tokio::test]
#[ntest::timeout(60000)]
async fn mtls_with_valid_client_cert_succeeds() -> io::Result<()> {
    let pki = generate_pki();
    let (server_addr, secret, shutdown) = spawn_server(&pki, Some(&pki.ca_cert)).await;

    let echo = spawn_tcp_echo().await;
    let client_cfg = build_quic_client_cfg(
        server_addr,
        Some(pki.ca_cert.clone()),
        Some((pki.client_cert.clone(), pki.client_key.clone())),
    );
    let quic_client = QuicClient::new(client_cfg).unwrap();

    let tunnel: TunnelClient<QuicClient, QuicConnection> =
        TunnelClient::new(quic_client, secret, None).await.unwrap();

    let dest: Address = echo.to_string().as_str().try_into().unwrap();
    let mut stream = tunnel.open_bidirectional(dest).await?;
    stream.write_all(b"mtls-ok!").await?;
    stream.flush().await?;
    let mut buf = [0u8; 8];
    stream.read_exact(&mut buf).await?;
    assert_eq!(&buf, b"mtls-ok!");

    let _ = shutdown.send(());
    Ok(())
}

#[tokio::test]
#[ntest::timeout(60000)]
async fn mtls_without_client_cert_rejected() {
    let pki = generate_pki();
    let (server_addr, secret, shutdown) = spawn_server(&pki, Some(&pki.ca_cert)).await;

    // No client cert — but server requires one.
    let client_cfg = build_quic_client_cfg(server_addr, Some(pki.ca_cert.clone()), None);
    let quic_client = QuicClient::new(client_cfg).unwrap();

    let result = TunnelClient::new(quic_client, secret, None).await;
    assert!(
        result.is_err(),
        "mTLS server should reject a client without a certificate"
    );

    let _ = shutdown.send(());
}

#[tokio::test]
#[ntest::timeout(60000)]
async fn mtls_with_wrong_client_cert_rejected() {
    let server_pki = generate_pki();
    let other_pki = generate_pki();
    let (server_addr, secret, shutdown) =
        spawn_server(&server_pki, Some(&server_pki.ca_cert)).await;

    // Client cert is signed by a CA the server does NOT trust.
    let client_cfg = build_quic_client_cfg(
        server_addr,
        Some(server_pki.ca_cert.clone()), // trusts the server
        Some((other_pki.client_cert.clone(), other_pki.client_key.clone())),
    );
    let quic_client = QuicClient::new(client_cfg).unwrap();

    let result = TunnelClient::new(quic_client, secret, None).await;
    assert!(
        result.is_err(),
        "mTLS server should reject a client cert signed by an untrusted CA"
    );

    let _ = shutdown.send(());
}
