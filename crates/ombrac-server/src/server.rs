use std::{io, sync::Arc};

use ombrac::prelude::*;
use ombrac_transport::{Acceptor, Reliable};

#[cfg(feature = "datagram")]
use ombrac_transport::Unreliable;

use ombrac_macros::{error, info};

use super::{Error, Result};

pub struct Server<T> {
    secret: Secret,
    transport: T,
}

impl<T> Server<T> {
    pub fn new(secret: Secret, transport: T) -> Self {
        Self { secret, transport }
    }
}

impl<T: Acceptor> Server<T> {
    #[inline]
    async fn handle_reliable(stream: impl Reliable, secret: Secret) -> Result<()> {
        Self::handle_connect(stream, secret).await
    }

    #[cfg(feature = "datagram")]
    #[inline]
    async fn handle_unreliable(datagram: impl Unreliable, secret: Secret) -> Result<()> {
        Self::handle_associate(datagram, secret).await
    }

    #[inline]
    async fn handle_connect(mut stream: impl Reliable, secret: Secret) -> Result<()> {
        use tokio::net::TcpStream;

        let request = Connect::from_async_read(&mut stream).await?;

        if request.secret != secret {
            return Err(Error::PermissionDenied);
        }

        let addr = request.address.to_socket_addr().await?;

        info!("Connect {}", addr);

        let mut target = TcpStream::connect(addr).await?;

        ombrac::io::util::copy_bidirectional(&mut stream, &mut target).await?;

        Ok(())
    }

    #[cfg(feature = "datagram")]
    #[inline]
    async fn handle_associate(datagram: impl Unreliable, secret: Secret) -> Result<()> {
        use std::net::SocketAddr;

        use bytes::Bytes;
        use tokio::net::UdpSocket;
        use tokio::time::{Duration, timeout};

        const DEFAULT_BUFFER_SIZE: usize = 2 * 1024;
        const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

        let local = SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], 0));
        let socket = UdpSocket::bind(local).await?;

        let socket_1 = Arc::new(socket);
        let socket_2 = Arc::clone(&socket_1);
        let datagram_1 = Arc::new(datagram);
        let datagram_2 = Arc::clone(&datagram_1);

        let mut handle_1 = tokio::spawn(async move {
            loop {
                let packet_result = timeout(IDLE_TIMEOUT, datagram_1.recv()).await;

                let mut bytes = match packet_result {
                    Ok(value) => value?,
                    Err(_) => return Ok(()), // Timeout
                };

                let packet = Associate::from_bytes(&mut bytes)?;
                if packet.secret != secret {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "Secret does not match",
                    ));
                };

                let target = packet.address.to_socket_addr().await?;
                socket_1.send_to(&packet.data, target).await?;
            }
        });

        let mut handle_2 = tokio::spawn(async move {
            let mut buf = [0u8; DEFAULT_BUFFER_SIZE];
            loop {
                match timeout(IDLE_TIMEOUT, socket_2.recv_from(&mut buf)).await {
                    Ok(Ok((n, addr))) => {
                        let data = Bytes::copy_from_slice(&buf[..n]);
                        let packet = Associate::with(secret, addr, data);

                        datagram_2.send(packet.to_bytes()?).await?;
                    }
                    Ok(Err(e)) => return Err(e),
                    Err(_) => break, // Timeout
                }
            }

            Ok(())
        });

        let result = tokio::select! {
            result = &mut handle_1 => {
                handle_2.abort();
                result
            },
            result = &mut handle_2 => {
                handle_1.abort();
                result
            },
        };

        match result {
            Ok(inner_result) => Ok(inner_result?),
            Err(e) if e.is_cancelled() => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn listen(self) -> Result<()> {
        let secret = self.secret;

        let transport = Arc::new(self.transport);

        #[cfg(feature = "datagram")]
        let mut datagram_handle = {
            let transport = Arc::clone(&transport);
            tokio::spawn(async move {
                loop {
                    match transport.accept_datagram().await {
                        Ok(datagram) => tokio::spawn(async move {
                            if let Err(_error) = Self::handle_unreliable(datagram, secret).await {
                                error!("{_error}");
                            }
                        }),

                        Err(error) => return error,
                    };
                }
            })
        };

        let mut stream_handle = {
            let transport = Arc::clone(&transport);
            tokio::spawn(async move {
                loop {
                    match transport.accept_bidirectional().await {
                        Ok(stream) => tokio::spawn(async move {
                            if let Err(_error) = Self::handle_reliable(stream, secret).await {
                                error!("{_error}");
                            }
                        }),
                        Err(error) => return error,
                    };
                }
            })
        };

        let error = {
            #[cfg(feature = "datagram")]
            {
                tokio::select! {
                    result = &mut stream_handle => {
                        datagram_handle.abort();
                        result?
                    },
                    result = &mut datagram_handle => {
                        stream_handle.abort();
                        result?
                    },
                }
            }

            #[cfg(not(feature = "datagram"))]
            {
                tokio::select! {
                    result = &mut stream_handle => {
                        result?
                    },
                }
            }
        };

        Err(Error::Io(error))
    }
}
