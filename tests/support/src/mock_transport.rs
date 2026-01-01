use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncWrite, DuplexStream, ReadBuf};
use tokio::sync::{Mutex, mpsc};

use ombrac_transport::{Acceptor, Connection, Initiator};

#[derive(Debug)]
pub struct MockStream(DuplexStream);

impl AsyncRead for MockStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for MockStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

#[derive(Clone)]
pub struct MockConnection {
    id: usize,
    remote_addr: SocketAddr,
    bidi_tx: mpsc::Sender<MockStream>,
    bidi_rx: Arc<Mutex<mpsc::Receiver<MockStream>>>,
    datagram_tx: mpsc::Sender<Bytes>,
    datagram_rx: Arc<Mutex<mpsc::Receiver<Bytes>>>,
}

impl Connection for MockConnection {
    type Stream = MockStream;

    fn id(&self) -> usize {
        self.id
    }

    fn remote_address(&self) -> io::Result<SocketAddr> {
        Ok(self.remote_addr)
    }

    fn max_datagram_size(&self) -> Option<usize> {
        // Use a reasonable MTU size that allows for fragmentation testing
        // This should be larger than the fragmented_overhead() (~277 bytes)
        Some(1500)
    }

    fn open_bidirectional(&self) -> impl Future<Output = io::Result<Self::Stream>> + Send {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        let tx = self.bidi_tx.clone();
        async move {
            tx.send(MockStream(server_stream))
                .await
                .map_err(|e| io::Error::new(io::ErrorKind::ConnectionReset, e.to_string()))?;
            Ok(MockStream(client_stream))
        }
    }

    fn accept_bidirectional(&self) -> impl Future<Output = io::Result<Self::Stream>> + Send {
        let rx = Arc::clone(&self.bidi_rx);
        async move {
            let mut guard = rx.lock().await;
            guard.recv().await.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "Acceptor has been dropped",
                )
            })
        }
    }

    fn send_datagram(&self, data: Bytes) -> impl Future<Output = io::Result<()>> + Send {
        let tx = self.datagram_tx.clone();
        async move {
            tx.send(data)
                .await
                .map_err(|e| io::Error::new(io::ErrorKind::ConnectionReset, e.to_string()))
        }
    }

    fn read_datagram(&self) -> impl Future<Output = io::Result<Bytes>> + Send {
        let rx = Arc::clone(&self.datagram_rx);
        async move {
            let mut guard = rx.lock().await;
            guard.recv().await.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "Connection has been dropped",
                )
            })
        }
    }

    fn close(&self, _error_code: u32, _reason: &[u8]) {}
}

fn create_connection_pair(
    client_addr: SocketAddr,
    server_addr: SocketAddr,
) -> (MockConnection, MockConnection) {
    let (c2s_bidi_tx, c2s_bidi_rx) = mpsc::channel(16);
    let (s2c_bidi_tx, s2c_bidi_rx) = mpsc::channel(16);
    let (c2s_dgram_tx, c2s_dgram_rx) = mpsc::channel(128);
    let (s2c_dgram_tx, s2c_dgram_rx) = mpsc::channel(128);

    let client_conn = MockConnection {
        id: 1,
        remote_addr: server_addr,
        bidi_tx: c2s_bidi_tx,
        bidi_rx: Arc::new(Mutex::new(s2c_bidi_rx)),
        datagram_tx: c2s_dgram_tx,
        datagram_rx: Arc::new(Mutex::new(s2c_dgram_rx)),
    };

    let server_conn = MockConnection {
        id: 1,
        remote_addr: client_addr,
        bidi_tx: s2c_bidi_tx,
        bidi_rx: Arc::new(Mutex::new(c2s_bidi_rx)),
        datagram_tx: s2c_dgram_tx,
        datagram_rx: Arc::new(Mutex::new(c2s_dgram_rx)),
    };

    (client_conn, server_conn)
}

pub struct MockInitiator {
    local_addr: SocketAddr,
    server_addr: SocketAddr,
    connection_tx: mpsc::Sender<MockConnection>,
}

impl Initiator for MockInitiator {
    type Connection = MockConnection;

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.local_addr)
    }

    async fn rebind(&self) -> io::Result<()> {
        Ok(())
    }

    fn connect(&self) -> impl Future<Output = io::Result<Self::Connection>> + Send {
        let (client_conn, server_conn) = create_connection_pair(self.local_addr, self.server_addr);
        let tx = self.connection_tx.clone();
        async move {
            tx.send(server_conn)
                .await
                .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e.to_string()))?;
            Ok(client_conn)
        }
    }
}

pub struct MockAcceptor {
    local_addr: SocketAddr,
    connection_rx: Mutex<mpsc::Receiver<MockConnection>>,
}

impl Acceptor for MockAcceptor {
    type Connection = MockConnection;

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.local_addr)
    }

    async fn accept(&self) -> io::Result<Self::Connection> {
        let mut guard = self.connection_rx.lock().await;
        guard
            .recv()
            .await
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "Acceptor channel closed"))
    }
}

pub fn mock_transport_pair() -> (MockInitiator, MockAcceptor) {
    let (tx, rx) = mpsc::channel(1);
    let client_addr: SocketAddr = "127.0.0.1:12345".parse().unwrap();
    let server_addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();

    let initiator = MockInitiator {
        local_addr: client_addr,
        server_addr,
        connection_tx: tx,
    };

    let acceptor = MockAcceptor {
        local_addr: server_addr,
        connection_rx: Mutex::new(rx),
    };

    (initiator, acceptor)
}
