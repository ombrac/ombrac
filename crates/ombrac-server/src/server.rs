use std::{io, sync::Arc};

use ombrac::prelude::*;
use ombrac_transport::{Reliable, Transport};

#[cfg(feature = "datagram")]
use ombrac_transport::Unreliable;

use ombrac_macros::error;

pub struct Server<T> {
    secret: Secret,
    transport: T,
}

impl<T: Transport> Server<T> {
    pub fn new(secret: Secret, transport: T) -> Self {
        Self { secret, transport }
    }

    async fn handle_reliable(stream: impl Reliable, secret: Secret) -> io::Result<()> {
        Self::handle_tcp_connect(stream, secret).await
    }

    #[cfg(feature = "datagram")]
    async fn handle_unreliable(stream: impl Unreliable, secret: Secret) -> io::Result<()> {
        Self::handle_udp_associate(stream, secret).await
    }

    #[inline]
    async fn handle_tcp_connect(mut stream: impl Reliable, secret: Secret) -> io::Result<()> {
        use tokio::net::TcpStream;

        let request = Connect::from_async_read(&mut stream).await?;

        if request.secret != secret {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Secret does not match",
            ));
        }

        let addr = request.address.to_socket_addr().await?;
        let mut target = TcpStream::connect(addr).await?;

        ombrac::io::util::copy_bidirectional(&mut stream, &mut target).await?;

        Ok(())
    }

    #[cfg(feature = "datagram")]
    #[inline]
    async fn handle_udp_associate(conn: impl Unreliable, secret: Secret) -> io::Result<()> {
        use std::net::SocketAddr;
        use tokio::net::UdpSocket;

        const DEFAULT_BUFFER_SIZE: usize = 2 * 1024;

        let local = SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], 0));
        let socket = UdpSocket::bind(local).await?;

        let sock_send = Arc::new(socket);
        let sock_recv = Arc::clone(&sock_send);
        let conn_send = Arc::new(conn);
        let conn_recv = Arc::clone(&conn_send);

        let handle = tokio::spawn(async move {
            let mut buf = [0u8; DEFAULT_BUFFER_SIZE];

            loop {
                let (len, addr) = sock_recv.recv_from(&mut buf).await?;

                let data = bytes::Bytes::copy_from_slice(&buf[..len]);
                let packet = Packet::with(secret, addr, data);

                if conn_send.send(packet.to_bytes()?).await.is_err() {
                    break;
                }
            }

            Ok::<(), io::Error>(())
        });

        while let Ok(mut packet) = conn_recv.recv().await {
            let packet = Packet::from_bytes(&mut packet)?;

            if packet.secret != secret {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Secret does not match",
                ));
            };

            let target = packet.address.to_socket_addr().await?;
            sock_send.send_to(&packet.data, target).await?;
        }

        handle.abort();

        Ok(())
    }

    pub async fn listen(self) -> io::Result<()> {
        let secret = self.secret.clone();

        let transport = Arc::new(self.transport);

        #[cfg(feature = "datagram")]
        {
            let unreliable_transport = transport.clone();
            tokio::spawn(async move {
                match unreliable_transport.unreliable().await {
                    Ok(stream) => {
                        tokio::spawn(async move {
                            if let Err(_error) = Self::handle_unreliable(stream, secret).await {
                                error!("{_error}");
                            }
                        });
                    }
                    Err(_error) => {
                        error!("{}", _error);
                    }
                };

                ()
            });
        }

        loop {
            match transport.reliable().await {
                Ok(stream) => tokio::spawn(async move {
                    if let Err(_error) = Self::handle_reliable(stream, secret).await {
                        error!("{_error}");
                    }
                }),
                Err(err) => return Err(io::Error::other(err.to_string())),
            };
        }
    }
}
