use std::future::Future;
use std::io;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::io::{IntoSplit, Streamable};
use crate::request::{Address, Request};

pub trait Client<Stream>: Send
where
    Stream: IntoSplit + AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
{
    fn outbound(&mut self) -> impl Future<Output = Option<Stream>> + Send;

    fn warp_tcp<S>(
        inbound: S,
        mut outbound: Stream,
        address: Address,
    ) -> impl Future<Output = io::Result<()>> + Send
    where
        S: IntoSplit + AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
    {
        use crate::io::utils::copy_bidirectional;

        async move {
            <Request as Streamable>::write(&Request::TcpConnect(address), &mut outbound).await?;

            copy_bidirectional(inbound, outbound).await?;

            Ok(())
        }
    }
}
