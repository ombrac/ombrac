use std::io;

use ombrac::prelude::*;
use ombrac_transport::{Initiator, Reliable};

#[cfg(feature = "datagram")]
use ombrac_transport::Unreliable;

pub struct Client<T> {
    secret: Secret,
    transport: T,
}

impl<T: Initiator> Client<T> {
    pub fn new(secret: Secret, transport: T) -> Self {
        Self { secret, transport }
    }

    #[inline]
    pub async fn connect<A>(&self, addr: A) -> io::Result<impl Reliable + '_>
    where
        A: Into<Address>,
    {
        use tokio::io::AsyncWriteExt;

        let mut stream = self.transport.open_bidirectional().await?;

        let request = Connect::with(self.secret, addr).to_bytes()?;
        stream.write_all(&request).await?;

        Ok(stream)
    }

    #[cfg(feature = "datagram")]
    #[inline]
    pub async fn associate(&self) -> io::Result<Datagram<impl Unreliable + '_>> {
        let stream = self.transport.open_datagram().await?;

        Ok(Datagram::with(self.secret, stream))
    }
}

#[cfg(feature = "datagram")]
pub struct Datagram<U: Unreliable>(Secret, U);

#[cfg(feature = "datagram")]
impl<U: Unreliable> Datagram<U> {
    fn with(secret: Secret, stream: U) -> Self {
        Self(secret, stream)
    }

    #[inline]
    pub async fn send<A, B>(&self, data: B, addr: A) -> io::Result<()>
    where
        A: Into<Address>,
        B: Into<bytes::Bytes>,
    {
        let packet = Associate::with(self.0, addr, data).to_bytes()?;

        self.1.send(packet).await
    }

    #[inline]
    pub async fn recv(&self) -> io::Result<(bytes::Bytes, Address)> {
        match self.1.recv().await {
            Ok(mut data) => {
                let packet = Associate::from_bytes(&mut data)?;
                Ok((packet.data, packet.address))
            }
            Err(error) => Err(error),
        }
    }
}
