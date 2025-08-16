use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio::sync::mpsc;

use ombrac::client::Client;
use ombrac::server::{SecretValid, Server};
use ombrac_transport::{Acceptor, Initiator, Reliable};

#[cfg(feature = "datagram")]
use {
    bytes::Bytes, ombrac::address::Address, ombrac::associate::Associate,
    ombrac::server::datagram::UdpHandlerConfig, ombrac_transport::Unreliable,
    tokio::net::UdpSocket,
};

const MOCK_BUFFER_SIZE: usize = 65535;

pub struct MockReliableStream(pub DuplexStream);

impl AsyncRead for MockReliableStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_read(cx, buf)
    }
}

impl AsyncWrite for MockReliableStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().0).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_shutdown(cx)
    }
}

impl Reliable for MockReliableStream {}

#[cfg(feature = "datagram")]
type DgramEndpoints = (mpsc::UnboundedSender<Bytes>, mpsc::UnboundedReceiver<Bytes>);

struct MockTransport {
    bidi_tx: mpsc::Sender<MockReliableStream>,
    bidi_rx: Arc<Mutex<mpsc::Receiver<MockReliableStream>>>,
    #[cfg(feature = "datagram")]
    dgram_tx: mpsc::Sender<DgramEndpoints>,
    #[cfg(feature = "datagram")]
    dgram_rx: Arc<Mutex<mpsc::Receiver<DgramEndpoints>>>,
}

struct MockInitiator {
    bidi_tx: mpsc::Sender<MockReliableStream>,
    #[cfg(feature = "datagram")]
    dgram_tx: mpsc::Sender<DgramEndpoints>,
}

struct MockAcceptor {
    bidi_rx: Arc<Mutex<mpsc::Receiver<MockReliableStream>>>,
    #[cfg(feature = "datagram")]
    dgram_rx: Arc<Mutex<mpsc::Receiver<DgramEndpoints>>>,
}

impl MockTransport {
    fn new() -> Self {
        let (bidi_tx, bidi_rx) = mpsc::channel(1);

        #[cfg(feature = "datagram")]
        let (dgram_tx, dgram_rx) = mpsc::channel(1);

        Self {
            bidi_tx,
            bidi_rx: Arc::new(Mutex::new(bidi_rx)),
            #[cfg(feature = "datagram")]
            dgram_tx,
            #[cfg(feature = "datagram")]
            dgram_rx: Arc::new(Mutex::new(dgram_rx)),
        }
    }

    fn split(self) -> (MockInitiator, MockAcceptor) {
        let initiator = MockInitiator {
            bidi_tx: self.bidi_tx,
            #[cfg(feature = "datagram")]
            dgram_tx: self.dgram_tx,
        };
        let acceptor = MockAcceptor {
            bidi_rx: self.bidi_rx,
            #[cfg(feature = "datagram")]
            dgram_rx: self.dgram_rx,
        };
        (initiator, acceptor)
    }
}

impl Initiator for MockInitiator {
    fn open_bidirectional(&self) -> impl Future<Output = io::Result<impl Reliable>> + Send {
        let (client_stream, server_stream) = tokio::io::duplex(MOCK_BUFFER_SIZE);
        let tx = self.bidi_tx.clone();
        async move {
            tx.send(MockReliableStream(server_stream))
                .await
                .map_err(|e| io::Error::other(e))?;
            Ok(MockReliableStream(client_stream))
        }
    }

    #[cfg(feature = "datagram")]
    fn open_datagram(&self) -> impl Future<Output = io::Result<impl Unreliable>> + Send {
        let tx = self.dgram_tx.clone();
        async move {
            let (client_tx, server_rx) = mpsc::unbounded_channel();
            let (server_tx, client_rx) = mpsc::unbounded_channel();

            tx.send((server_tx, server_rx))
                .await
                .map_err(|_| io::Error::other("failed to send datagram channel"))?;

            Ok(MockDatagram {
                tx: client_tx,
                rx: Arc::new(Mutex::new(client_rx)),
            })
        }
    }
}

impl Acceptor for MockAcceptor {
    fn accept_bidirectional(&self) -> impl Future<Output = io::Result<impl Reliable>> + Send {
        let arc = self.bidi_rx.clone();
        async move {
            let mut rx = arc.lock().await;
            rx.recv().await.ok_or(io::Error::other("no stream"))
        }
    }

    #[cfg(feature = "datagram")]
    fn accept_datagram(&self) -> impl Future<Output = io::Result<impl Unreliable>> + Send {
        let arc = self.dgram_rx.clone();
        async move {
            let mut rx_guard = arc.lock().await;
            if let Some((server_tx, server_rx)) = rx_guard.recv().await {
                Ok(MockDatagram {
                    tx: server_tx,
                    rx: Arc::new(Mutex::new(server_rx)),
                })
            } else {
                Err(io::Error::other("datagram channel closed"))
            }
        }
    }
}

#[cfg(feature = "datagram")]
struct MockDatagram {
    tx: mpsc::UnboundedSender<Bytes>,
    rx: Arc<Mutex<mpsc::UnboundedReceiver<Bytes>>>,
}

#[cfg(feature = "datagram")]
impl Unreliable for MockDatagram {
    fn send(&self, data: Bytes) -> impl Future<Output = io::Result<()>> + Send {
        async move {
            self.tx
                .send(data)
                .map_err(|e| io::Error::other(e.to_string()))
        }
    }

    async fn recv(&self) -> io::Result<Bytes> {
        let arc = self.rx.clone();
        let mut rx = arc.lock().await;
        rx.recv().await.ok_or(io::Error::other("channel closed"))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_connect_and_tcp_proxy() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target_addr = listener.local_addr().unwrap();

    let echo_server_handle = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0; 1024];
        let n = socket.read(&mut buf).await.unwrap();
        socket.write_all(&buf[..n]).await.unwrap();
    });

    let secret = [255u8; 32];
    let validator = SecretValid(secret);
    let (initiator, acceptor) = MockTransport::new().split();
    let client = Client::new(initiator);
    let server = Server::new(acceptor);

    let server_handle = tokio::spawn(async move {
        let stream = server.accept_connect().await.unwrap();
        Server::handle_connect(&validator, stream).await.unwrap();
    });

    let mut client_stream = client.connect(target_addr, secret).await.unwrap();

    let request_data = b"hello world";
    client_stream.write_all(request_data).await.unwrap();

    let mut response_buf = vec![0; request_data.len()];
    client_stream.read_exact(&mut response_buf).await.unwrap();

    assert_eq!(request_data.as_slice(), response_buf.as_slice());

    server_handle.await.unwrap();
    echo_server_handle.await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "datagram")]
async fn test_associate_and_udp_proxy() {
    let target_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let target_addr = target_socket.local_addr().unwrap();

    let echo_server_handle = tokio::spawn(async move {
        let mut buf = [0; 1024];
        let (n, from) = target_socket.recv_from(&mut buf).await.unwrap();
        target_socket.send_to(&buf[..n], from).await.unwrap();
    });

    let secret = [0u8; 32];
    let validator = SecretValid(secret);
    let (initiator, acceptor) = MockTransport::new().split();
    let client = Client::new(initiator);
    let server = Server::new(acceptor);

    let server_handle = tokio::spawn(async move {
        let datagram = server.accept_associate().await.unwrap();
        Server::handle_associate(&validator, datagram, UdpHandlerConfig::default().into())
            .await
            .unwrap();
    });

    let client_datagram = client.associate().await.unwrap();

    let payload = Bytes::from_static(b"ping");
    let request_packet = Associate::with(secret, target_addr, payload.clone());

    client_datagram.send(request_packet).await.unwrap();

    let response_packet = client_datagram.recv().await.unwrap();

    assert_eq!(response_packet.data, payload);
    assert_eq!(response_packet.address, Address::from(target_addr));

    server_handle.abort();
    echo_server_handle.await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_connect_with_invalid_secret() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target_addr = listener.local_addr().unwrap();

    let _echo_server_handle = tokio::spawn(async move {
        let _ = listener.accept().await;
    });

    let secret = [255u8; 32];
    let invalid_secret = [111u8; 32];
    let validator = SecretValid(secret);
    let (initiator, acceptor) = MockTransport::new().split();
    let client = Client::new(initiator);
    let server = Server::new(acceptor);

    let server_handle = tokio::spawn(async move {
        let stream = server.accept_connect().await.unwrap();
        assert!(Server::handle_connect(&validator, stream).await.is_err());
    });

    let result = client.connect(target_addr, invalid_secret).await;

    assert!(result.is_ok());

    server_handle.await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[cfg(feature = "datagram")]
async fn test_associate_with_invalid_secret() {
    let target_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let target_addr = target_socket.local_addr().unwrap();

    let echo_server_handle = tokio::spawn(async move {
        let mut buf = [0; 1024];
        let (n, from) = target_socket.recv_from(&mut buf).await.unwrap();
        target_socket.send_to(&buf[..n], from).await.unwrap();
    });

    let secret = [0u8; 32];
    let invalid_secret = [111u8; 32];
    let validator = SecretValid(secret);
    let (initiator, acceptor) = MockTransport::new().split();
    let client = Client::new(initiator);
    let server = Server::new(acceptor);

    let server_handle = tokio::spawn(async move {
        let datagram = server.accept_associate().await.unwrap();
        let result =
            Server::handle_associate(&validator, datagram, UdpHandlerConfig::default().into())
                .await;
        assert!(result.is_err());
    });

    let client_datagram = client.associate().await.unwrap();
    let payload = Bytes::from_static(b"ping");
    let request_packet = Associate::with(invalid_secret, target_addr, payload.clone());

    client_datagram.send(request_packet).await.unwrap();

    let result = client_datagram.recv().await;

    assert!(result.is_err());

    server_handle.await.unwrap();
    echo_server_handle.abort();
}
