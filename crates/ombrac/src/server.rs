use std::io;
use std::net::SocketAddr;

use tokio::net::TcpStream;

use ombrac_transport::{Acceptor, Reliable};

use crate::Secret;
use crate::client::Stream;
use crate::connect::Connect;

/// Represents a server that accepts connections from a transport layer.
///
/// It uses a generic `Acceptor` trait to remain transport-agnostic.
pub struct Server<T> {
    transport: T,
}

impl<T: Acceptor> Server<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    pub async fn accept_connect(&self) -> io::Result<Stream<impl Reliable>> {
        let mut stream = self.transport.accept_bidirectional().await?;
        let connect = Connect::from_async_read(&mut stream).await?;
        Ok(Stream(stream, connect))
    }
}

impl Server<()> {
    pub async fn handle_connect<V, R>(
        validator: &V,
        mut stream: Stream<R>,
    ) -> io::Result<(u64, u64)>
    where
        V: Validator,
        R: Reliable + Send + Sync + 'static,
    {
        let target = stream.1.address.to_socket_addr().await?;
        validator
            .is_valid(stream.1.secret, Some(target), None)
            .await?;

        let mut tcp_stream = TcpStream::connect(target).await?;

        crate::io::util::copy_bidirectional(&mut stream.0, &mut tcp_stream).await
    }
}

#[cfg(feature = "datagram")]
pub mod datagram {
    use std::sync::Arc;
    use std::time::Duration;

    use bytes::Bytes;
    use ombrac_transport::{Acceptor, Unreliable};

    use tokio::net::UdpSocket;
    use tokio::time::timeout;

    use crate::associate::Associate;
    use crate::client::Datagram;

    use super::*;

    pub struct UdpHandlerConfig {
        pub idle_timeout: Duration,
        pub buffer_size: usize,
    }

    impl Default for UdpHandlerConfig {
        fn default() -> Self {
            Self {
                idle_timeout: Duration::from_secs(30),
                buffer_size: 1500,
            }
        }
    }

    impl<T: Acceptor> Server<T> {
        #[cfg(feature = "datagram")]
        pub async fn accept_associate(&self) -> io::Result<Datagram<impl Unreliable>> {
            let datagram = self.transport.accept_datagram().await?;
            Ok(Datagram(datagram))
        }
    }

    impl Server<()> {
        #[cfg(feature = "datagram")]
        pub async fn handle_associate<V, U>(
            validator: &V,
            datagram: Datagram<U>,
            config: Arc<UdpHandlerConfig>,
        ) -> io::Result<()>
        where
            V: Validator,
            U: Unreliable + Send + Sync + 'static,
        {
            let first_packet = match timeout(config.idle_timeout, datagram.recv()).await {
                Ok(Ok(packet)) => packet,
                Ok(Err(e)) => return Err(e),
                Err(_) => return Ok(()),
            };
            let session_secret = first_packet.secret;

            validator.is_valid(session_secret, None, None).await?;

            let initial_target = first_packet.address.to_socket_addr().await?;
            let bind_addr = match initial_target {
                SocketAddr::V4(_) => SocketAddr::from(([0, 0, 0, 0], 0)),
                SocketAddr::V6(_) => SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], 0)),
            };

            let datagram = Arc::new(datagram);
            let udp_socket = Arc::new(UdpSocket::bind(bind_addr).await?);

            udp_socket
                .send_to(&first_packet.data, initial_target)
                .await?;

            let client_to_target_task = tokio::spawn(proxy_client_to_target(
                Arc::clone(&datagram),
                Arc::clone(&udp_socket),
                config.clone(),
            ));

            let target_to_client_task = tokio::spawn(proxy_target_to_client(
                datagram,
                udp_socket,
                session_secret,
                config,
            ));

            let (client_res, target_res) =
                tokio::join!(client_to_target_task, target_to_client_task);

            client_res??;
            target_res??;

            Ok(())
        }
    }

    async fn proxy_client_to_target<U>(
        datagram: Arc<Datagram<U>>,
        udp_socket: Arc<UdpSocket>,
        config: Arc<UdpHandlerConfig>,
    ) -> io::Result<()>
    where
        U: Unreliable,
    {
        loop {
            let packet = match timeout(config.idle_timeout, datagram.recv()).await {
                Ok(Ok(packet)) => packet,
                Ok(Err(e)) => return Err(e),
                Err(_) => break,
            };
            let target = packet.address.to_socket_addr().await?;
            udp_socket.send_to(&packet.data, target).await?;
        }
        Ok(())
    }

    async fn proxy_target_to_client<U>(
        datagram: Arc<Datagram<U>>,
        udp_socket: Arc<UdpSocket>,
        session_secret: Secret,
        config: Arc<UdpHandlerConfig>,
    ) -> io::Result<()>
    where
        U: Unreliable,
    {
        let mut buf = vec![0u8; config.buffer_size];
        loop {
            let (n, from_addr) =
                match timeout(config.idle_timeout, udp_socket.recv_from(&mut buf)).await {
                    Ok(Ok(result)) => result,
                    Ok(Err(e)) => return Err(e),
                    Err(_) => break,
                };

            let response_packet =
                Associate::with(session_secret, from_addr, Bytes::copy_from_slice(&buf[..n]));

            datagram.send(response_packet).await?;
        }
        Ok(())
    }
}

pub trait Validator {
    fn is_valid(
        &self,
        secret: Secret,
        target: Option<SocketAddr>,
        from: Option<SocketAddr>,
    ) -> impl Future<Output = io::Result<()>> + Send;
}

#[derive(Clone, Copy)]
pub struct SecretValid(pub Secret);

impl Validator for SecretValid {
    async fn is_valid(
        &self,
        secret: Secret,
        _: Option<SocketAddr>,
        _: Option<SocketAddr>,
    ) -> io::Result<()> {
        if secret != self.0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "invalid secret",
            ));
        }

        Ok(())
    }
}
