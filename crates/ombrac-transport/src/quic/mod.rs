mod stream;

pub mod client;
pub mod error;
pub mod server;

use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use std::{fs, io};

use quinn::{IdleTimeout, VarInt};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde::{Deserialize, Serialize};

type Result<T> = std::result::Result<T, error::Error>;

pub use quinn::Connection;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Congestion {
    Bbr,
    Cubic,
    NewReno,
}

impl FromStr for Congestion {
    type Err = error::Error;
    fn from_str(value: &str) -> Result<Self> {
        match value.to_lowercase().as_str() {
            "bbr" => Ok(Congestion::Bbr),
            "cubic" => Ok(Congestion::Cubic),
            "newreno" => Ok(Congestion::NewReno),
            _ => Err(Self::Err::InvalidCongestion),
        }
    }
}

#[derive(Debug, Default)]
pub struct TransportConfig(pub(crate) quinn::TransportConfig);

impl TransportConfig {
    pub fn congestion(
        &mut self,
        congestion: Congestion,
        initial_window: Option<u64>,
    ) -> Result<&mut Self> {
        use quinn::congestion;

        let congestion: Arc<dyn congestion::ControllerFactory + Send + Sync + 'static> =
            match congestion {
                Congestion::Bbr => {
                    let mut config = congestion::BbrConfig::default();
                    if let Some(value) = initial_window {
                        config.initial_window(value);
                    }
                    Arc::new(config)
                }
                Congestion::Cubic => {
                    let mut config = congestion::CubicConfig::default();
                    if let Some(value) = initial_window {
                        config.initial_window(value);
                    }
                    Arc::new(config)
                }
                Congestion::NewReno => {
                    let mut config = congestion::NewRenoConfig::default();
                    if let Some(value) = initial_window {
                        config.initial_window(value);
                    }
                    Arc::new(config)
                }
            };

        self.0.congestion_controller_factory(congestion);
        Ok(self)
    }

    pub fn max_idle_timeout(&mut self, value: Duration) -> Result<&mut Self> {
        self.0.max_idle_timeout(Some(IdleTimeout::try_from(value)?));
        Ok(self)
    }

    pub fn keep_alive_period(&mut self, value: Duration) -> Result<&mut Self> {
        self.0.keep_alive_interval(Some(value));
        Ok(self)
    }

    pub fn max_open_bidirectional_streams(&mut self, value: u64) -> Result<&mut Self> {
        self.0.max_concurrent_bidi_streams(VarInt::try_from(value)?);
        Ok(self)
    }
}

fn load_certificates(path: &Path) -> io::Result<Vec<CertificateDer<'static>>> {
    let content = fs::read(path)?;
    let certs = if path.extension().is_some_and(|ext| ext == "der") {
        vec![CertificateDer::from(content)]
    } else {
        rustls_pemfile::certs(&mut &*content).collect::<io::Result<Vec<_>>>()?
    };
    Ok(certs)
}

fn load_private_key(path: &Path) -> io::Result<PrivateKeyDer<'static>> {
    let content = fs::read(path)?;
    let key = if path.extension().is_some_and(|ext| ext == "der") {
        PrivateKeyDer::Pkcs8(content.into())
    } else {
        rustls_pemfile::private_key(&mut &*content)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no private key found in PEM file")
        })?
    };
    Ok(key)
}

#[derive(Debug)]
pub enum ConnectionError {
    QuinnConnection(quinn::ConnectionError),
    QuinnSendDatagram(quinn::SendDatagramError),
}

impl From<quinn::ConnectionError> for ConnectionError {
    fn from(e: quinn::ConnectionError) -> Self {
        ConnectionError::QuinnConnection(e)
    }
}

impl From<quinn::SendDatagramError> for ConnectionError {
    fn from(e: quinn::SendDatagramError) -> Self {
        ConnectionError::QuinnSendDatagram(e)
    }
}

impl From<ConnectionError> for io::Error {
    fn from(e: ConnectionError) -> Self {
        match e {
            ConnectionError::QuinnConnection(error) => {
                use quinn::ConnectionError::*;
                let kind = match error {
                    LocallyClosed | ConnectionClosed(_) | ApplicationClosed(_) | Reset => {
                        io::ErrorKind::ConnectionReset
                    }
                    TimedOut => io::ErrorKind::TimedOut,
                    _ => io::ErrorKind::Other,
                };
                io::Error::new(kind, error)
            }
            ConnectionError::QuinnSendDatagram(error) => {
                use quinn::SendDatagramError::*;
                let kind = match error {
                    ConnectionLost(_) => io::ErrorKind::ConnectionReset,
                    _ => io::ErrorKind::Other,
                };
                io::Error::new(kind, error)
            }
        }
    }
}

impl crate::Connection for quinn::Connection {
    type Stream = stream::Stream;

    async fn accept_bidirectional(&self) -> io::Result<Self::Stream> {
        let (send, recv) = quinn::Connection::accept_bi(self)
            .await
            .map_err(ConnectionError::from)?;
        Ok(stream::Stream(send, recv))
    }

    async fn open_bidirectional(&self) -> io::Result<Self::Stream> {
        let (send, recv) = quinn::Connection::open_bi(self)
            .await
            .map_err(ConnectionError::from)?;
        Ok(stream::Stream(send, recv))
    }

    #[cfg(feature = "datagram")]
    async fn read_datagram(&self) -> io::Result<bytes::Bytes> {
        quinn::Connection::read_datagram(self)
            .await
            .map_err(|e| ConnectionError::from(e).into())
    }

    #[cfg(feature = "datagram")]
    async fn send_datagram(&self, data: bytes::Bytes) -> io::Result<()> {
        quinn::Connection::send_datagram(self, data)
            .map_err(|e| ConnectionError::from(e).into())
    }

    fn remote_address(&self) -> io::Result<std::net::SocketAddr> {
        Ok(quinn::Connection::remote_address(self))
    }

    #[cfg(feature = "datagram")]
    fn max_datagram_size(&self) -> Option<usize> {
        quinn::Connection::max_datagram_size(self)
    }

    fn id(&self) -> usize {
        quinn::Connection::stable_id(self)
    }

    fn close(&self, error_code: u32, reason: &[u8]) {
        self.close(error_code.into(), reason);
    }
}
