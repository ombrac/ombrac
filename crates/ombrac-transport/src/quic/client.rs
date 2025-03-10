use std::{io, net::SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::io::Result;

use quinn::IdleTimeout;

use super::{Connection, stream::Stream};

pub struct Builder {
    bind: Option<String>,

    server_name: Option<String>,
    server_address: String,

    tls_cert: Option<PathBuf>,
    tls_skip: bool,

    enable_zero_rtt: bool,
    enable_connection_multiplexing: bool,
    congestion_initial_window: Option<u64>,

    max_idle_timeout: Option<Duration>,
    max_keep_alive_period: Option<Duration>,
    max_open_bidirectional_streams: Option<u64>,
}

impl Builder {
    pub fn new(server_address: String) -> Self {
        Builder {
            bind: None,
            server_name: None,
            server_address,
            tls_cert: None,
            tls_skip: false,
            enable_zero_rtt: false,
            enable_connection_multiplexing: false,
            congestion_initial_window: None,
            max_idle_timeout: None,
            max_keep_alive_period: None,
            max_open_bidirectional_streams: None,
        }
    }

    pub fn with_server_name(mut self, value: String) -> Self {
        self.server_name = Some(value);
        self
    }

    pub fn with_bind(mut self, value: String) -> Self {
        self.bind = Some(value);
        self
    }

    pub fn with_tls_cert(mut self, value: PathBuf) -> Self {
        self.tls_cert = Some(value);
        self
    }

    pub fn with_tls_skip(mut self, value: bool) -> Self {
        self.tls_skip = value;
        self
    }

    pub fn with_enable_zero_rtt(mut self, value: bool) -> Self {
        self.enable_zero_rtt = value;
        self
    }

    pub fn with_enable_connection_multiplexing(mut self, value: bool) -> Self {
        self.enable_connection_multiplexing = value;
        self
    }

    pub fn with_congestion_initial_window(mut self, value: u64) -> Self {
        self.congestion_initial_window = Some(value);
        self
    }

    pub fn with_max_idle_timeout(mut self, value: Duration) -> Self {
        self.max_idle_timeout = Some(value);
        self
    }

    pub fn with_max_keep_alive_period(mut self, value: Duration) -> Self {
        self.max_keep_alive_period = Some(value);
        self
    }

    pub fn with_max_open_bidirectional_streams(mut self, value: u64) -> Self {
        self.max_open_bidirectional_streams = Some(value);
        self
    }

    pub async fn build(self) -> Result<Connection> {
        Connection::with_client(self).await
    }

    fn server_name(&self) -> Result<&str> {
        match &self.server_name {
            Some(value) => Ok(value),
            None => {
                let pos = self
                    .server_address
                    .rfind(':')
                    .ok_or(format!("invalid server address {}", self.server_address)).map_err(|e| io::Error::other(e.to_string()))?;

                Ok(&self.server_address[..pos])
            }
        }
    }

    async fn bind_address(&self) -> Result<SocketAddr> {
        use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6};

        let address = match &self.bind {
            Some(value) => value.parse().map_err(|e| io::Error::other(e))?,
            None => match self.server_address().await? {
                SocketAddr::V4(_) => SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)),
                SocketAddr::V6(_) => {
                    SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0))
                }
            },
        };

        Ok(address)
    }

    async fn server_address(&self) -> Result<SocketAddr> {
        use tokio::net::lookup_host;

        let address = lookup_host(&self.server_address)
            .await?
            .next()
            .ok_or(format!(
                "failed to resolve server address '{}'",
                self.server_address
            )).map_err(|e| io::Error::other(e.to_string()))?;

        Ok(address)
    }
}

impl Connection {
    async fn with_client(config: Builder) -> Result<Self> {
        let tls_config = {
            use rustls::{ClientConfig, RootCertStore};

            let mut roots = RootCertStore::empty();

            if let Some(path) = &config.tls_cert {
                let certs = super::load_certificates(path)?;
                roots.add_parsable_certificates(certs);
            } else {
                roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            }

            let mut tls_config = ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth();

            tls_config.alpn_protocols = [b"h3"].iter().map(|&x| x.into()).collect();

            if config.tls_skip {
                tls_config
                    .dangerous()
                    .set_certificate_verifier(Arc::new(cert_verifier::NullVerifier));
            }

            if config.enable_zero_rtt {
                tls_config.enable_early_data = true;
            }

            tls_config
        };

        let quic_config = {
            use quinn::crypto::rustls::QuicClientConfig;

            let config = QuicClientConfig::try_from(tls_config).map_err(|e| io::Error::other(e.to_string()))?;

            config
        };

        let client_config = {
            use quinn::{congestion, ClientConfig, TransportConfig, VarInt};

            let mut transport = TransportConfig::default();
            let mut congestion = congestion::BbrConfig::default();

            if let Some(value) = config.congestion_initial_window {
                congestion.initial_window(value);
            }

            if let Some(value) = config.max_idle_timeout {
                transport.max_idle_timeout(Some(IdleTimeout::try_from(value).map_err(|e| io::Error::other(e.to_string()))?));
            }

            if let Some(value) = config.max_keep_alive_period {
                transport.keep_alive_interval(Some(value));
            }

            if let Some(value) = config.max_open_bidirectional_streams {
                transport.max_concurrent_bidi_streams(VarInt::from_u64(value).map_err(|e| io::Error::other(e.to_string()))?);
            }

            transport.congestion_controller_factory(Arc::new(congestion));

            let mut config = ClientConfig::new(Arc::new(quic_config));
            config.transport_config(Arc::new(transport));

            config
        };

        let endpoint = {
            use quinn::Endpoint;

            let bind_address = config.bind_address().await?;

            let mut endpoint = Endpoint::client(bind_address)?;
            endpoint.set_default_client_config(client_config);

            endpoint
        };

        let server_name = config.server_name()?.to_string();
        let server_address = config.server_address().await?;

        let enable_zero_rtt = config.enable_zero_rtt;
        let enable_connection_multiplexing = config.enable_connection_multiplexing;

        let tcp_endpoint = endpoint.clone();
        let tcp_server_name = server_name.clone();

        let (sender, receiver) = async_channel::bounded(1);

        #[cfg(feature = "datagram")]
        let (datagram_sender, datagram_receiver) = async_channel::bounded(1);

        let handle = tokio::spawn(async move {
            use ombrac_macros::error;

            tokio::spawn(async move {
                'connection: loop {
                    let connection = match connection(
                        &tcp_endpoint,
                        server_address,
                        &tcp_server_name,
                        enable_zero_rtt,
                    )
                    .await
                    {
                        Ok(value) => value,
                        Err(_error) => {
                            error!("{}", _error);
                            continue 'connection;
                        }
                    };

                    'stream: loop {
                        let stream = match connection.open_bi().await {
                            Ok(value) => value,
                            Err(_error) => {
                                error!("{}", _error);
                                break 'stream;
                            }
                        };

                        if sender.send(Stream(stream.0, stream.1)).await.is_err() {
                            break 'connection;
                        }

                        if !enable_connection_multiplexing {
                            break 'stream;
                        }
                    }
                }
            });

            #[cfg(feature = "datagram")]
            tokio::spawn(async move {
                use crate::quic::datagram::Session;

                'connection: loop {
                    let connection =
                        match connection(&endpoint, server_address, &server_name, enable_zero_rtt)
                            .await
                        {
                            Ok(value) => value,
                            Err(_error) => {
                                error!("{}", _error);
                                continue 'connection;
                            }
                        };

                    let session = Session::with_client(connection);

                    'datagram: loop {
                        let datagram = match session.open_bidirectional().await {
                            Some(value) => value,
                            None => break 'datagram,
                        };

                        if datagram_sender.send(datagram).await.is_err() {
                            break 'connection;
                        };
                    }
                }
            });
        });

        Ok(Connection {
            handle,
            stream: receiver,
            #[cfg(feature = "datagram")]
            datagram: datagram_receiver,
        })
    }
}

#[inline]
async fn connection(
    endpoint: &quinn::Endpoint,
    addr: SocketAddr,
    name: &str,
    enable_zero_rtt: bool,
) -> Result<quinn::Connection> {
    let connecting = endpoint.connect(addr, name).map_err(|e| io::Error::other(e.to_string()))?;

    let connection = if enable_zero_rtt {
        match connecting.into_0rtt() {
            Ok((conn, zero_rtt_accepted)) => {
                if !zero_rtt_accepted.await {
                    return Err(io::Error::other("Zero rtt not accepted"));
                }

                conn
            }
            Err(conn) => conn.await?,
        }
    } else {
        connecting.await?
    };

    Ok(connection)
}

mod cert_verifier {
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::Error;
    use rustls::{DigitallySignedStruct, SignatureScheme};

    #[derive(Debug)]
    pub struct NullVerifier;

    impl ServerCertVerifier for NullVerifier {
        fn verify_server_cert(
            &self,
            _end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp_response: &[u8],
            _now: UnixTime,
        ) -> Result<ServerCertVerified, Error> {
            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn verify_tls13_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            vec![
                SignatureScheme::RSA_PKCS1_SHA1,
                SignatureScheme::ECDSA_SHA1_Legacy,
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
