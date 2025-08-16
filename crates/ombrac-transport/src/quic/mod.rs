#[cfg(feature = "datagram")]
mod datagram;
mod error;
mod stream;

pub mod client;
pub mod server;

use std::path::Path;
use std::{fs, io};

use rustls::pki_types::{CertificateDer, PrivateKeyDer};

type Result<T> = std::result::Result<T, error::Error>;

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
