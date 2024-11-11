use std::future::Future;

use bytes::BytesMut;
use tokio::io::{AsyncReadExt, AsyncWriteExt, Result};

const DEFAULT_BUF_SIZE: usize = 16 * 1024;

pub trait ToBytes {
    fn to_bytes(&self) -> BytesMut;
}

pub trait Streamable {
    fn write<T>(&self, stream: &mut T) -> impl Future<Output = Result<()>> + Send
    where
        Self: ToBytes + Send + Sync,
        T: AsyncWriteExt + Unpin + Send,
    {
        async move { stream.write_all(&self.to_bytes()).await }
    }

    fn read<T>(stream: &mut T) -> impl Future<Output = Result<Self>> + Send
    where
        Self: Sized,
        T: AsyncReadExt + Unpin + Send;
}

pub trait IntoSplit {
    fn into_split(
        self,
    ) -> (
        impl AsyncReadExt + Unpin + Send,
        impl AsyncWriteExt + Unpin + Send,
    );
}

pub async fn copy_bidirectional<A, B>(a: A, b: B) -> Result<()>
where
    A: IntoSplit,
    B: IntoSplit,
{
    let (mut a_reader, mut a_writer) = a.into_split();
    let (mut b_reader, mut b_writer) = b.into_split();

    let a_to_b = async {
        let mut buffer = [0u8; DEFAULT_BUF_SIZE];
        loop {
            let n = match a_reader.read(&mut buffer).await {
                Ok(0) => return Ok(()),
                Ok(n) => n,
                Err(err) => return Err(err),
            };
            b_writer.write_all(&buffer[..n]).await?;
            b_writer.flush().await?;
        }
    };

    let b_to_a = async {
        let mut buffer = [0u8; DEFAULT_BUF_SIZE];
        loop {
            let n = match b_reader.read(&mut buffer).await {
                Ok(0) => return Ok(()),
                Ok(n) => n,
                Err(err) => return Err(err),
            };
            a_writer.write_all(&buffer[..n]).await?;
            a_writer.flush().await?;
        }
    };

    tokio::select! {
        result = a_to_b => {result},
        result = b_to_a => {result},
    }
}

mod impl_tokio {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    use super::IntoSplit;

    impl IntoSplit for TcpStream {
        fn into_split(
            self,
        ) -> (
            impl AsyncReadExt + Unpin + Send,
            impl AsyncWriteExt + Unpin + Send,
        ) {
            self.into_split()
        }
    }
}

#[cfg(feature = "s2n-quic")]
mod impl_s2n_quic {
    use s2n_quic::stream::BidirectionalStream;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::IntoSplit;

    impl IntoSplit for BidirectionalStream {
        fn into_split(
            self,
        ) -> (
            impl AsyncReadExt + Unpin + Send,
            impl AsyncWriteExt + Unpin + Send,
        ) {
            self.split()
        }
    }
}
