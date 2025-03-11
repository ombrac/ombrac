use std::{io, sync::Arc};

use ombrac::prelude::*;
use ombrac_transport::{Acceptor, Reliable};

#[cfg(feature = "datagram")]
use ombrac_transport::Unreliable;

use ombrac_macros::error;

pub struct Server<T> {
    secret: Secret,
    transport: T,
}

impl<T: Acceptor> Server<T> {
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
        use tokio::time::{timeout, Duration};

        const DEFAULT_BUFFER_SIZE: usize = 2 * 1024;
        const RECV_TIMEOUT: Duration = Duration::from_secs(180);

        let local = SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], 0));
        let socket = UdpSocket::bind(local).await?;
        let sock_send = Arc::new(socket);
        let sock_recv = Arc::clone(&sock_send);
        let conn_send = Arc::new(conn);
        let conn_recv = Arc::clone(&conn_send);

        let mut recv_handle = tokio::spawn(async move {
            let mut buf = [0u8; DEFAULT_BUFFER_SIZE];
            loop {
                let (len, addr) = sock_recv.recv_from(&mut buf).await?;
                let data = bytes::Bytes::copy_from_slice(&buf[..len]);
                let packet = Packet::with(secret, addr, data);
                if let Err(e) = conn_send.send(packet.to_bytes()?).await {
                    return Err(io::Error::other(e.to_string()));
                }
            }
        });

        let mut send_handle = tokio::spawn(async move {
            loop {
                let packet_result = timeout(RECV_TIMEOUT, conn_recv.recv()).await;

                let result = match packet_result {
                    Ok(value) => value,
                    Err(_) => return Ok(()), // UDP recv timeout
                };

                match result {
                    Ok(mut packet) => {
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
                    Err(e) => {
                        return Err(io::Error::other(e.to_string()));
                    }
                }
            }
        });

        let result = tokio::select! {
            result = &mut recv_handle => {
                send_handle.abort();
                result
            },
            result = &mut send_handle => {
                recv_handle.abort();
                result
            },
        };

        match result {
            Ok(inner_result) => inner_result,
            Err(e) if e.is_cancelled() => Ok(()),
            Err(e) => Err(io::Error::new(io::ErrorKind::Other, e)),
        }
    }

    pub async fn listen(self) -> io::Result<()> {
        let secret = self.secret.clone();

        let transport = Arc::new(self.transport);

        #[cfg(feature = "datagram")]
        let datagram_handle = {
            let transport = transport.clone();
            let datagram_handle = tokio::spawn(async move {
                loop {
                    match transport.accept_datagram().await {
                        Ok(stream) => {
                            tokio::spawn(async move {
                                if let Err(_error) = Self::handle_unreliable(stream, secret).await {
                                    error!("{_error}");
                                }
                            });
                        }
                        Err(_error) => {
                            error!("{_error}");

                            break;
                        }
                    };
                }
            });

            datagram_handle
        };

        loop {
            match transport.accept_bidirectional().await {
                Ok(stream) => tokio::spawn(async move {
                    if let Err(_error) = Self::handle_reliable(stream, secret).await {
                        error!("{_error}");
                    }
                }),
                Err(err) => {
                    #[cfg(feature = "datagram")]
                    datagram_handle.abort();

                    return Err(io::Error::other(err.to_string()));
                }
            };
        }
    }
}
