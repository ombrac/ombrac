use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use arc_swap::{ArcSwap, Guard};
use ombrac_macros::{debug, error, warn};
use quinn::ClientConfig;
use quinn::crypto::rustls::QuicClientConfig;
use tokio::sync::Mutex;

#[cfg(feature = "datagram")]
use crate::Unreliable;
#[cfg(feature = "datagram")]
use crate::quic::datagram::Session;
use crate::quic::stream::Stream;
use crate::{Initiator, Reliable};

use super::Result;
use super::TransportConfig;

#[derive(Debug)]
pub struct Builder {
    bind_addr: SocketAddr,
    server_name: String,
    server_addr: SocketAddr,
    tls_cert: Option<PathBuf>,
    tls_skip: bool,
    enable_zero_rtt: bool,
    enable_connection_multiplexing: bool,
}

impl Builder {
    pub fn new(server_addr: SocketAddr, server_name: String) -> Self {
        let bind_addr = match server_addr {
            SocketAddr::V4(_) => SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0),
            SocketAddr::V6(_) => SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), 0),
        };
        Self {
            bind_addr,
            server_addr,
            server_name,
            tls_cert: None,
            tls_skip: false,
            enable_zero_rtt: false,
            enable_connection_multiplexing: true,
        }
    }

    pub fn with_server_name(&mut self, value: String) -> &mut Self {
        self.server_name = value;
        self
    }

    pub fn with_bind(&mut self, value: SocketAddr) -> &mut Self {
        self.bind_addr = value;
        self
    }

    pub fn with_tls(&mut self, value: PathBuf) -> &mut Self {
        self.tls_cert = Some(value);
        self
    }

    pub fn with_tls_skip(&mut self, value: bool) -> &mut Self {
        self.tls_skip = value;
        self
    }

    pub fn with_enable_zero_rtt(&mut self, value: bool) -> &mut Self {
        self.enable_zero_rtt = value;
        self
    }

    pub fn with_enable_connection_multiplexing(&mut self, value: bool) -> &mut Self {
        self.enable_connection_multiplexing = value;
        self
    }

    pub fn with_bind_addr(mut self, addr: SocketAddr) -> Self {
        self.bind_addr = addr;
        self
    }

    pub async fn build(self, transport_config: TransportConfig) -> Result<QuicClient> {
        let client_config = self.build_client_config(transport_config.0)?;
        let mut endpoint = quinn::Endpoint::client(self.bind_addr)?;
        endpoint.set_default_client_config(client_config);

        let connection = endpoint
            .connect(self.server_addr, &self.server_name)?
            .await
            .map_err(io::Error::other)?;

        Ok(QuicClient {
            config: self,
            endpoint,
            connection: ArcSwap::new(
                Connection {
                    inner: connection.clone(),
                    #[cfg(feature = "datagram")]
                    datagram_session: Session::with_client(connection),
                }
                .into(),
            ),
            reconnect_lock: Mutex::new(()),
        })
    }

    fn build_client_config(
        &self,
        transport_config: quinn::TransportConfig,
    ) -> Result<ClientConfig> {
        let client_crypto = self.build_tls_config()?;
        let mut client_config =
            quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(client_crypto)?));

        client_config.transport_config(Arc::new(transport_config));

        Ok(client_config)
    }

    fn build_tls_config(&self) -> Result<rustls::ClientConfig> {
        let mut roots = rustls::RootCertStore::empty();
        if let Some(path) = &self.tls_cert {
            let certs = super::load_certificates(path)?;
            roots.add_parsable_certificates(certs);
        } else {
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        }

        let mut config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();

        config.alpn_protocols = vec![b"h3".to_vec()];

        if self.tls_skip {
            warn!("TLS certificate verification is DISABLED - this is not secure!");
            config
                .dangerous()
                .set_certificate_verifier(Arc::new(cert_verifier::NullVerifier));
        }

        if self.enable_zero_rtt {
            config.enable_early_data = true;
        }

        Ok(config)
    }
}

pub struct Connection {
    inner: quinn::Connection,
    #[cfg(feature = "datagram")]
    datagram_session: Session,
}

pub struct QuicClient {
    config: Builder,
    endpoint: quinn::Endpoint,
    connection: ArcSwap<Connection>,
    reconnect_lock: Mutex<()>,
}

impl Initiator for QuicClient {
    async fn open_bidirectional(&self) -> io::Result<impl Reliable> {
        let conn_arc = self.connection.load();
        if !self.config.enable_connection_multiplexing {
            let (send, recv) = self.reconnect_and_open_bi(conn_arc).await?;
            Ok(Stream(send, recv))
        } else {
            match conn_arc.inner.open_bi().await {
                Ok((send, recv)) => Ok(Stream(send, recv)),
                Err(quinn::ConnectionError::ApplicationClosed(_))
                | Err(quinn::ConnectionError::ConnectionClosed(_))
                | Err(quinn::ConnectionError::LocallyClosed)
                | Err(quinn::ConnectionError::Reset)
                | Err(quinn::ConnectionError::TimedOut) => {
                    warn!("QUIC connection lost, attempting to reconnect");
                    let (send, recv) = self.reconnect_and_open_bi(conn_arc).await?;
                    Ok(Stream(send, recv))
                }
                Err(e) => {
                    error!("Unexpected QUIC connection error: {:?}", e);
                    return Err(io::Error::other(e));
                }
            }
        }
    }

    #[cfg(feature = "datagram")]
    async fn open_datagram(&self) -> io::Result<impl Unreliable> {
        let conn_arc = self.connection.load();
        conn_arc
            .datagram_session
            .open_datagram()
            .await
            .ok_or(io::Error::other("connection closed"))
    }
}

impl QuicClient {
    async fn reconnect_and_open_bi(
        &self,
        old_conn_arc: Guard<Arc<Connection>>,
    ) -> io::Result<(quinn::SendStream, quinn::RecvStream)> {
        let _lock = self.reconnect_lock.lock().await;

        if !Arc::ptr_eq(&*old_conn_arc, &*self.connection.load()) {
            return self
                .connection
                .load()
                .inner
                .open_bi()
                .await
                .map_err(io::Error::other);
        }

        let new_connection = { self.connect().await? };

        self.connection.store(Arc::new(Connection {
            inner: new_connection.clone(),
            #[cfg(feature = "datagram")]
            datagram_session: Session::with_client(new_connection),
        }));

        self.connection
            .load()
            .inner
            .open_bi()
            .await
            .map_err(io::Error::other)
    }

    async fn connect(&self) -> io::Result<quinn::Connection> {
        let mut _is_zero_rtt = false;
        let connecting = self
            .endpoint
            .connect(self.config.server_addr, &self.config.server_name)
            .map_err(io::Error::other)?;

        let connection = if self.config.enable_zero_rtt {
            match connecting.into_0rtt() {
                Ok((conn, zero_rtt_accepted)) => {
                    if !zero_rtt_accepted.await {
                        return Err(io::Error::other("Zero RTT not accept"));
                    }

                    _is_zero_rtt = true;

                    conn
                }
                Err(conn) => conn.await?,
            }
        } else {
            connecting.await?
        };

        debug!(
            "QUIC Connection{} established with {} at {}",
            match _is_zero_rtt {
                true => "(0-RTT)",
                _ => "",
            },
            &self.config.server_name,
            &self.config.server_addr
        );

        Ok(connection)
    }
}

mod cert_verifier {
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::{DigitallySignedStruct, SignatureScheme};

    #[derive(Debug)]
    pub struct NullVerifier;

    impl ServerCertVerifier for NullVerifier {
        fn verify_server_cert(
            &self,
            _: &CertificateDer<'_>,
            _: &[CertificateDer<'_>],
            _: &ServerName<'_>,
            _: &[u8],
            _: UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            Ok(ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(
            &self,
            _: &[u8],
            _: &CertificateDer<'_>,
            _: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }
        fn verify_tls13_signature(
            &self,
            _: &[u8],
            _: &CertificateDer<'_>,
            _: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }
        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            vec![
                SignatureScheme::RSA_PKCS1_SHA1,
                SignatureScheme::RSA_PKCS1_SHA256,
                SignatureScheme::ECDSA_NISTP256_SHA256,
                SignatureScheme::RSA_PKCS1_SHA384,
                SignatureScheme::ECDSA_NISTP384_SHA384,
                SignatureScheme::RSA_PKCS1_SHA512,
                SignatureScheme::ECDSA_NISTP521_SHA512,
                SignatureScheme::RSA_PSS_SHA256,
                SignatureScheme::RSA_PSS_SHA384,
                SignatureScheme::RSA_PSS_SHA512,
                SignatureScheme::ED25519,
                SignatureScheme::ED448,
            ]
        }
    }
}
