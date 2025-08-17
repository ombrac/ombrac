#[cfg(feature = "datagram")]
mod datagram;
mod error;
mod stream;

pub mod client;
pub mod server;

use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use std::{fs, io};

use quinn::{IdleTimeout, VarInt};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

type Result<T> = std::result::Result<T, error::Error>;

#[derive(Debug, Clone, Copy)]
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

#[derive(Default)]
pub struct TransportConfig(pub(crate) quinn::TransportConfig);

impl TransportConfig {
    pub fn with_congestion(
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

    pub fn with_max_idle_timeout(&mut self, value: Duration) -> Result<&mut Self> {
        self.0.max_idle_timeout(Some(IdleTimeout::try_from(value)?));
        Ok(self)
    }

    pub fn with_max_keep_alive_period(&mut self, value: Duration) -> Result<&mut Self> {
        self.0.keep_alive_interval(Some(value));
        Ok(self)
    }

    pub fn with_max_open_bidirectional_streams(&mut self, value: u64) -> Result<&mut Self> {
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
