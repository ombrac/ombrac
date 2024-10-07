use std::marker::PhantomData;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt, Result};

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
    RS: AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
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
            tokio::spawn(async move { Self::handle(stream, &resolver).await });
        }
    }

    async fn handle(mut stream: RS, resolver: &RE) -> Result<()> {
        use crate::request::Request;
        use crate::response::Response;
        use crate::Streamable;

        let request = <Request as Streamable>::read(&mut stream).await?;

        match request {
            Request::TcpConnect(address) => {
                use tokio::io::copy_bidirectional;
                use tokio::net::TcpStream;

                let address = address.to_socket_address(resolver).await?;
                let mut connect = TcpStream::connect(address).await?;

                Streamable::write(&Response::Succeed, &mut stream).await?;

                copy_bidirectional(&mut stream, &mut connect).await?
            }
        };

        Ok(())
    }
}
