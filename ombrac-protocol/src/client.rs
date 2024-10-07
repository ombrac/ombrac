use std::marker::PhantomData;

use tokio::io::{AsyncReadExt, AsyncWriteExt, Result};

use crate::request::Request;
use crate::Provider;

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
    RS: AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
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

        let response = <Response as Streamable>::read(&mut remote).await?;

        if let Response::Succeed = response {
            copy_bidirectional(&mut local, &mut remote).await?;
        };

        Ok(())
    }
}
