use std::marker::PhantomData;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt, Result};

use crate::io::{IntoSplit, Streamable};
use crate::request::Request;
use crate::{Provider, Resolver};

pub struct Server<R, RE, RS> {
    accept: R,
    resolver: Arc<RE>,
    _accept_stream: PhantomData<RS>,
}

impl<R, RS, RE> Server<R, RE, RS>
where
    R: Provider<RS>,
    RE: Resolver + Send + Sync + 'static,
    RS: IntoSplit + AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
{
    pub fn with(accept: R, resolver: RE) -> Self {
        Self {
            accept,
            resolver: resolver.into(),
            _accept_stream: PhantomData,
        }
    }

    pub async fn start(&mut self) {
        while let Some(stream) = self.accept.fetch().await {
            let resolver = self.resolver.clone();
            tokio::spawn(async move { Self::handler(stream, &resolver).await });
        }
    }

    async fn handler(mut stream: RS, resolver: &RE) -> Result<()> {
        use crate::io::utils::copy_bidirectional;
        use tokio::net::TcpStream;

        let request = <Request as Streamable>::read(&mut stream).await?;

        match request {
            Request::TcpConnect(address) => {
                let addr = address.to_socket_address(resolver).await?;
                let connect = TcpStream::connect(addr).await?;

                copy_bidirectional(stream, connect).await?
            }

            #[cfg(feature = "udp")]
            Request::UdpAssociate(_) => udp_associate::relay(stream, resolver).await?,
        };

        Ok(())
    }
}

#[cfg(feature = "udp")]
mod udp_associate {
    use std::io::Error;
    use std::net::SocketAddr;

    use tokio::net::UdpSocket;
    use tokio::sync::mpsc::{self, Receiver, Sender};

    use crate::request::udp::Datagram;
    use crate::request::Address;

    use super::*;

    pub async fn relay(stream: impl IntoSplit, resolver: &impl Resolver) -> Result<()> {
        let (read_stream, write_stream) = stream.into_split();
        let (request_sender, mut request_receiver) = mpsc::channel(1);
        let (response_sender, mut response_receiver) = mpsc::channel(1);

        let outbound_v4 = UdpSocket::bind("0.0.0.0:0").await?;
        let outbound_v6 = UdpSocket::bind("[::]:0").await?;

        tokio::try_join!(
            relay_request(read_stream, request_sender),
            relay_response(write_stream, &mut response_receiver),
            handle_request(&outbound_v4, &outbound_v6, &mut request_receiver, resolver),
            handle_response(&outbound_v4, &response_sender),
            handle_response(&outbound_v6, &response_sender),
        )?;

        Ok(())
    }

    async fn relay_request<S>(mut stream: S, sender: Sender<Datagram>) -> Result<()>
    where
        S: AsyncReadExt + Unpin + Send,
    {
        loop {
            let datagram = <Datagram as Streamable>::read(&mut stream).await?;
            if sender.send(datagram).await.is_err() {
                break;
            }
        }

        Err(Error::other("failed to send datagram"))
    }

    async fn handle_request(
        outbound_v4: &UdpSocket,
        outbound_v6: &UdpSocket,
        receiver: &mut Receiver<Datagram>,
        resolver: &impl Resolver,
    ) -> Result<()> {
        while let Some(datagram) = receiver.recv().await {
            let target_address = datagram.address.to_socket_address(resolver).await?;

            let outbound = match &target_address {
                SocketAddr::V4(_) => outbound_v4,
                SocketAddr::V6(_) => outbound_v6,
            };

            outbound.send_to(&datagram.data, target_address).await?;
        }

        Err(Error::other("failed to receive datagram"))
    }

    async fn relay_response<S>(mut stream: S, receiver: &mut Receiver<Datagram>) -> Result<()>
    where
        S: AsyncWriteExt + Unpin + Send,
    {
        while let Some(datagram) = receiver.recv().await {
            <Datagram as Streamable>::write(&datagram, &mut stream).await?;
        }

        Err(Error::other("failed to receive datagram"))
    }

    async fn handle_response(outbound: &UdpSocket, sender: &Sender<Datagram>) -> Result<()> {
        let mut buffer = vec![0u8; 4096];

        loop {
            let (size, address) = outbound.recv_from(&mut buffer).await?;
            let data = (&buffer[..size]).into();

            let datagram = Datagram {
                address: Address::from_socket_address(address),
                length: size as u16,
                data,
            };

            if sender.send(datagram).await.is_err() {
                break;
            }
        }

        Err(Error::other("failed to send datagram"))
    }
}
