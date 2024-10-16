use std::io::{Error, Result};
use std::marker::PhantomData;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::request::Request;
use crate::{IntoSplit, Provider, Streamable};

pub struct Client<L, R, LS, RS> {
    local: L,
    remote: R,
    _local_stream: PhantomData<LS>,
    _remote_stream: PhantomData<RS>,
}

impl<L, R, LS, RS> Client<L, R, LS, RS>
where
    L: Provider<(LS, Request)>,
    R: Provider<RS>,
    LS: AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
    RS: IntoSplit + AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
{
    pub fn with(local: L, remote: R) -> Self {
        Self {
            local,
            remote,
            _local_stream: PhantomData,
            _remote_stream: PhantomData,
        }
    }

    pub async fn start(&mut self) {
        while let Some((local, request)) = self.local.fetch().await {
            if let Some(remote) = self.remote.fetch().await {
                tokio::spawn(async move { Self::handler(local, remote, request).await });
            }
        }
    }

    async fn handler(mut local: LS, mut remote: RS, request: Request) -> Result<()> {
        use tokio::io::copy_bidirectional;

        Streamable::write(&request, &mut remote).await?;

        match request {
            Request::TcpConnect(_) => {
                copy_bidirectional(&mut local, &mut remote).await?;
            }

            #[cfg(feature = "udp")]
            Request::UdpAssociate(channel) => {
                if let Some((sender, receiver)) = channel {
                    tokio::select! {
                        result = local.read_u8() => { result?; }
                        result = udp_associate::relay(remote, sender, receiver) => { result?; }
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(feature = "udp")]
mod udp_associate {
    use tokio::sync::mpsc::{Receiver, Sender};

    use super::*;
    use crate::request::udp::Datagram;

    pub async fn relay(
        stream: impl IntoSplit,
        response_sender: Sender<Datagram>,
        mut request_receiver: Receiver<Datagram>,
    ) -> Result<()> {
        let (read_stream, write_stream) = stream.into_split();

        tokio::try_join!(
            relay_request(write_stream, &mut request_receiver),
            relay_response(read_stream, &response_sender),
        )?;

        Ok(())
    }

    async fn relay_request<S>(mut stream: S, receiver: &mut Receiver<Datagram>) -> Result<()>
    where
        S: AsyncWriteExt + Unpin + Send,
    {
        while let Some(datagram) = receiver.recv().await {
            <Datagram as Streamable>::write(&datagram, &mut stream).await?
        }

        Ok(())
    }

    async fn relay_response<S>(mut stream: S, sender: &Sender<Datagram>) -> Result<()>
    where
        S: AsyncReadExt + Unpin + Send,
    {
        loop {
            let datagram = <Datagram as Streamable>::read(&mut stream).await?;
            if let Err(_err) = sender.send(datagram).await {
                return Err(Error::other("failed to send datagram"));
            }
        }
    }
}
