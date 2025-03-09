use std::io;

use bytes::{Buf, BufMut, Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::address::Address;
use crate::{Secret, SECRET_LENGTH};

pub struct Connect {
    pub secret: Secret,
    pub address: Address,
}

impl Connect {
    pub fn with<A: Into<Address>>(secret: Secret, addr: A) -> Self {
        Self {
            secret,
            address: addr.into(),
        }
    }

    pub fn to_bytes(&self) -> io::Result<Bytes> {
        let address_bytes = self.address.to_bytes()?;

        let total_len = SECRET_LENGTH + address_bytes.len();
        let mut buffer = BytesMut::with_capacity(total_len);

        buffer.put_slice(&self.secret);
        buffer.put_slice(&address_bytes);

        Ok(buffer.freeze())
    }

    pub fn from_bytes<B: Buf>(buf: &mut B) -> io::Result<Self> {
        if buf.remaining() < SECRET_LENGTH {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Insufficient data for secret",
            ));
        }

        let mut secret = [0u8; SECRET_LENGTH];
        buf.copy_to_slice(&mut secret);

        let address = Address::from_bytes(buf)?;

        Ok(Self { secret, address })
    }

    pub async fn from_async_read<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Self> {
        let mut secret = [0u8; SECRET_LENGTH];
        reader.read_exact(&mut secret).await?;

        let address = Address::from_async_read(reader).await?;

        Ok(Self { secret, address })
    }
}
