use std::marker::PhantomData;

use tokio::io::{AsyncReadExt, AsyncWriteExt, Result};

use crate::request::Request;
use crate::{IntoSplit, Provider};

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
                tokio::spawn(async move { Self::handle(local, remote, request).await });
            }
        }
    }

    async fn handle(mut local: LS, mut remote: RS, request: Request) -> Result<()> {
        use tokio::io::copy_bidirectional;

        use crate::response::Response;
        use crate::Streamable;

        Streamable::write(&request, &mut remote).await?;

        match request {
            Request::TcpConnect(_) => {
                let response = <Response as Streamable>::read(&mut remote).await?;

                if let Response::Succeed = response {
                    copy_bidirectional(&mut local, &mut remote).await?;
                };
            }

            #[cfg(feature = "udp")]
            Request::UdpAssociate(datagram) => {
                if let Some((sender, receiver)) = datagram {
                    tokio::select! {
                        _ = local.read_u8() => {}
                        _ = udp_associate::relay(remote, sender, receiver) => {}
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

    use crate::request::udp::Datagram;
    use crate::Streamable;

    use super::*;

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
            if sender.send(datagram).await.is_err() {
                return Ok(());
            }
        }
    }
}
