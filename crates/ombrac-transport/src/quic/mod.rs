use std::path::PathBuf;
use std::{fs, io};

use async_channel::Receiver;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

pub mod client;
pub mod server;

pub struct Connection(Receiver<Stream>);
pub struct Stream(quinn::SendStream, quinn::RecvStream);

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

impl ombrac::Provider for Connection {
    type Item = Stream;

    async fn fetch(&self) -> Option<Self::Item> {
        self.0.recv().await.ok()
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

#[cfg(test)]
mod tests {
    use super::{client, server, Connection, Stream};
    use std::{net::SocketAddr, time::Duration};
    use tests_support::cert::CertificateGenerator;
    use tests_support::net::find_available_udp_addr;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const TIMEOUT: Duration = Duration::from_secs(1);
    const STARTUP_WAIT: Duration = Duration::from_millis(300);

    async fn setup_connections(
        listen_addr: SocketAddr,
        zero_rtt: bool,
        enable_multiplexing: bool,
    ) -> (Connection, Connection) {
        eprintln!("{}", listen_addr.to_string());
        tokio::time::sleep(STARTUP_WAIT).await;

        let addr_str = listen_addr.to_string();
        let (cert_path, key_path) = CertificateGenerator::generate();

        let server_conn = server::Builder::new(addr_str.clone(), cert_path.clone(), key_path)
            .with_enable_zero_rtt(zero_rtt)
            .build()
            .await
            .expect("Failed to build server connection");

        tokio::time::sleep(STARTUP_WAIT).await;

        let client_conn = client::Builder::new(addr_str)
            .with_server_name("localhost".to_string())
            .with_tls_cert(cert_path)
            .with_enable_zero_rtt(zero_rtt)
            .with_enable_connection_multiplexing(enable_multiplexing)
            .build()
            .await
            .expect("Failed to build client connection");

        (server_conn, client_conn)
    }

    async fn fetch_stream(conn: &Connection) -> Stream {
        use ombrac::Provider;
        tokio::time::timeout(TIMEOUT, conn.fetch())
            .await
            .expect("Timed out waiting for stream")
            .expect("Failed to fetch stream")
    }

    #[tokio::test]
    async fn test_client_server_connection() {
        let listen_addr = find_available_udp_addr("127.0.0.1".parse().unwrap());
        let (server_conn, client_conn) = setup_connections(listen_addr, false, false).await;

        let mut client_stream = fetch_stream(&client_conn).await;
        let msg = b"hello quic";
        client_stream.write_all(msg).await.unwrap();

        let mut server_stream = fetch_stream(&server_conn).await;
        let mut buf = vec![0u8; msg.len()];
        server_stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, msg);
    }

    #[tokio::test]
    async fn test_client_server_connection_zerortt() {
        let listen_addr = find_available_udp_addr("127.0.0.1".parse().unwrap());
        let (server_conn, client_conn) = setup_connections(listen_addr, true, false).await;

        let mut client_stream = fetch_stream(&client_conn).await;
        let msg = b"hello zerortt";
        client_stream.write_all(msg).await.unwrap();

        let mut server_stream = fetch_stream(&server_conn).await;
        let mut buf = vec![0u8; msg.len()];
        server_stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, msg);
    }

    #[tokio::test]
    async fn test_multiplexed_streams() {
        let listen_addr = find_available_udp_addr("127.0.0.1".parse().unwrap());
        let (server_conn, client_conn) = setup_connections(listen_addr, false, true).await;

        let mut client_stream1 = fetch_stream(&client_conn).await;
        let msg1 = b"stream 1";
        client_stream1.write_all(msg1).await.unwrap();

        let mut server_stream1 = fetch_stream(&server_conn).await;
        let mut buf1 = vec![0u8; msg1.len()];
        server_stream1.read_exact(&mut buf1).await.unwrap();
        assert_eq!(&buf1, msg1);

        let mut client_stream2 = fetch_stream(&client_conn).await;
        let msg2 = b"stream 2";
        client_stream2.write_all(msg2).await.unwrap();

        let mut server_stream2 = fetch_stream(&server_conn).await;
        let mut buf2 = vec![0u8; msg2.len()];
        server_stream2.read_exact(&mut buf2).await.unwrap();
        assert_eq!(&buf2, msg2);
    }

    #[tokio::test]
    async fn test_bidirectional_data_exchange() {
        let listen_addr = find_available_udp_addr("127.0.0.1".parse().unwrap());
        let (server_conn, client_conn) = setup_connections(listen_addr, false, false).await;

        let mut client_stream = fetch_stream(&client_conn).await;
        let client_msg = b"hello from client";
        client_stream.write_all(client_msg).await.unwrap();

        let mut server_stream = fetch_stream(&server_conn).await;
        let mut server_buf = vec![0u8; client_msg.len()];
        server_stream.read_exact(&mut server_buf).await.unwrap();
        assert_eq!(&server_buf, client_msg);

        let server_reply = b"hello from server";
        server_stream.write_all(server_reply).await.unwrap();
        let mut client_buf = vec![0u8; server_reply.len()];
        client_stream.read_exact(&mut client_buf).await.unwrap();
        assert_eq!(&client_buf, server_reply);
    }
}
