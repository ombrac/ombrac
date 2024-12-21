use std::path::PathBuf;
use std::{fs, io};

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

pub mod client;
pub mod server;

pub struct Connection(tokio::sync::mpsc::Receiver<Stream>);
pub struct Stream(quinn::SendStream, quinn::RecvStream);

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

impl ombrac::Provider for Connection {
    type Item = Stream;

    async fn fetch(&mut self) -> Option<Self::Item> {
        self.0.recv().await
    }
}

fn load_certificates(path: &PathBuf) -> io::Result<Vec<CertificateDer<'static>>> {
    let cert_chain = fs::read(path)?;

    let result = if path.extension().is_some_and(|x| x == "der") {
        vec![CertificateDer::from(cert_chain)]
    } else {
        rustls_pemfile::certs(&mut &*cert_chain).collect::<std::result::Result<_, _>>()?
    };

    Ok(result)
}

fn load_private_key(path: &PathBuf) -> io::Result<PrivateKeyDer<'static>> {
    let key = fs::read(path)?;

    let result = if path.extension().is_some_and(|x| x == "der") {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key))
    } else {
        match rustls_pemfile::private_key(&mut &*key)? {
            Some(value) => value,
            None => return Err(io::Error::other("error")),
        }
    };

    Ok(result)
}

mod impl_tokio_io {
    use std::io;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use tokio::io::{AsyncRead, AsyncWrite};

    use super::Stream;

    impl AsyncRead for Stream {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            AsyncRead::poll_read(Pin::new(&mut self.get_mut().1), cx, buf)
        }
    }

    impl AsyncWrite for Stream {
        fn poll_write(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<Result<usize, io::Error>> {
            AsyncWrite::poll_write(Pin::new(&mut self.get_mut().0), cx, buf)
        }

        fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
            AsyncWrite::poll_flush(Pin::new(&mut self.get_mut().0), cx)
        }

        fn poll_shutdown(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), io::Error>> {
            AsyncWrite::poll_shutdown(Pin::new(&mut self.get_mut().0), cx)
        }
    }
}
