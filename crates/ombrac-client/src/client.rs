#[cfg(feature = "datagram")]
use bytes::Bytes;
use ombrac::prelude::*;
#[cfg(feature = "datagram")]
use ombrac_transport::Unreliable;
use ombrac_transport::{Initiator, Reliable};

use crate::Result;

pub struct Client<T> {
    secret: Secret,
    transport: T,
}

impl<T> Client<T> {
    pub fn new(secret: Secret, transport: T) -> Self {
        Self { secret, transport }
    }
}

impl<T: Initiator> Client<T> {
    #[inline]
    pub async fn connect<A>(&self, addr: A) -> Result<impl Reliable>
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
    pub async fn associate(&self) -> Result<Datagram<impl Unreliable>> {
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
    pub async fn send<A, B>(&self, data: B, addr: A) -> Result<()>
    where
        A: Into<Address>,
        B: Into<Bytes>,
    {
        let packet = Associate::with(self.0, addr, data).to_bytes()?;

        Ok(self.1.send(packet).await?)
    }

    #[inline]
    pub async fn recv(&self) -> Result<(Bytes, Address)> {
        let mut data = self.1.recv().await?;
        let packet = Associate::from_bytes(&mut data)?;

        Ok((packet.data, packet.address))
    }
}
