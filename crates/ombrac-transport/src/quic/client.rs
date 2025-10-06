use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::Arc;

use ombrac_macros::{debug, info, warn};

use super::Result;
use crate::quic::TransportConfig;

#[derive(Debug, Clone)]
pub struct Config {
    pub server_name: String,
    pub server_addr: SocketAddr,

    pub enable_zero_rtt: bool,
    pub alpn_protocols: Vec<Vec<u8>>,
    pub skip_server_verification: bool,
    pub root_ca_path: Option<PathBuf>,
    pub client_cert_key_paths: Option<(PathBuf, PathBuf)>,

    transport_config: Arc<quinn::TransportConfig>,
}

impl Config {
    pub fn new(server_addr: SocketAddr, server_name: String) -> Self {
        Self {
            server_name,
            server_addr,
            root_ca_path: None,
            client_cert_key_paths: None,
            skip_server_verification: false,
            enable_zero_rtt: false,
            alpn_protocols: Vec::new(),
            transport_config: Arc::new(quinn::TransportConfig::default()),
        }
    }

    pub fn transport_config(&mut self, config: TransportConfig) {
        self.transport_config = Arc::new(config.0)
    }

    fn build_endpoint_config(&self) -> Result<quinn::EndpointConfig> {
        Ok(quinn::EndpointConfig::default())
    }

    fn build_client_config(&self) -> Result<quinn::ClientConfig> {
        use quinn::crypto::rustls::QuicClientConfig;

        let crypto_config = Arc::new(QuicClientConfig::try_from(self.build_tls_config()?)?);

        let mut client_config = quinn::ClientConfig::new(crypto_config);
        client_config.transport_config(self.transport_config.clone());

        Ok(client_config)
    }

    fn build_tls_config(&self) -> Result<rustls::ClientConfig> {
        let mut roots = rustls::RootCertStore::empty();
        if let Some(path) = &self.root_ca_path {
            let certs = super::load_certificates(path)?;
            roots.add_parsable_certificates(certs);
        } else {
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        }

        let config_builder = rustls::ClientConfig::builder().with_root_certificates(roots);

        let mut tls_config = if let Some((cert_path, key_path)) = &self.client_cert_key_paths {
            let client_certs = super::load_certificates(cert_path)?;
            let client_key = super::load_private_key(key_path)?;
            config_builder.with_client_auth_cert(client_certs, client_key)?
        } else {
            config_builder.with_no_client_auth()
        };

        tls_config.alpn_protocols = self.alpn_protocols.clone();

        if self.skip_server_verification {
            warn!("TLS certificate verification is DISABLED - this is not secure!");
            tls_config
                .dangerous()
                .set_certificate_verifier(Arc::new(cert_verifier::NullVerifier));
        }

        if self.enable_zero_rtt {
            tls_config.enable_early_data = true;
        }

        Ok(tls_config)
    }
}

pub struct Client {
    config: Config,
    endpoint: quinn::Endpoint,
}

impl Client {
    pub fn new(socket: UdpSocket, config: Config) -> Result<Self> {
        let client_config = config.build_client_config()?;
        let endpoint_config = config.build_endpoint_config()?;

        let runtime =
            quinn::default_runtime().ok_or_else(|| io::Error::other("No async runtime found"))?;
        let mut endpoint = quinn::Endpoint::new_with_abstract_socket(
            endpoint_config,
            None,
            runtime.wrap_udp_socket(socket)?,
            runtime,
        )?;
        endpoint.set_default_client_config(client_config);

        Ok(Self { config, endpoint })
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.endpoint.local_addr()?)
    }

    async fn connect(&self) -> Result<quinn::Connection> {
        let connecting = self
            .endpoint
            .connect(self.config.server_addr, &self.config.server_name)?;

        let (connection, _is_zero_rtt_accepted) = if self.config.enable_zero_rtt {
            match connecting.into_0rtt() {
                Ok((conn, zero_rtt_accepted)) => {
                    let is_accepted = zero_rtt_accepted.await;
                    if !is_accepted {
                        warn!("Zero-RTT connection not accepted by server");
                    }

                    (conn, is_accepted)
                }
                Err(conn) => {
                    debug!("Zero-RTT not available, using regular connection");
                    (conn.await?, false)
                }
            }
        } else {
            (connecting.await?, false)
        };

        info!(
            "Connection established with {} at {} (0-RTT: {})",
            &self.config.server_name, &self.config.server_addr, _is_zero_rtt_accepted
        );

        Ok(connection)
    }
}

impl crate::Initiator for Client {
    type Connection = quinn::Connection;

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Client::local_addr(self).map_err(io::Error::other)
    }

    async fn connect(&self) -> io::Result<Self::Connection> {
        Client::connect(self).await.map_err(io::Error::other)
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
            rustls::crypto::aws_lc_rs::default_provider()
                .signature_verification_algorithms
                .supported_schemes()
                .to_vec()
        }
    }
}
