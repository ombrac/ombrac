use std::io::Result;

use bytes::{BufMut, BytesMut};
use tokio::io::AsyncReadExt;

use crate::{Streamable, ToBytes};

#[rustfmt::skip]
mod consts {
    pub const SUCCEED: u8 = 0x01;
    pub const NO_ACCEPTABLE_REQUEST: u8 = 0xFF;
}

pub enum Response {
    Succeed,
    NoAcceptableMethod,
}

impl ToBytes for Response {
    fn to_bytes(&self) -> BytesMut {
        let mut bytes = BytesMut::new();

        match self {
            Self::Succeed => {
                bytes.put_u8(consts::SUCCEED);
            }
            Self::NoAcceptableMethod => bytes.put_u8(consts::NO_ACCEPTABLE_REQUEST),
        };

        bytes
    }
}

impl Streamable for Response {
    async fn read<T>(stream: &mut T) -> Result<Self>
    where
        T: AsyncReadExt + Unpin + Send,
    {
        let response = match stream.read_u8().await? {
            consts::SUCCEED => Self::Succeed,
            _ => Self::NoAcceptableMethod,
        };

        Ok(response)
    }
}
