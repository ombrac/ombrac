mod v5;

use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;

use ombrac::request::Address;
use ombrac::Provider;
use ombrac_macros::{info, try_or_return};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};

use crate::Client;

pub struct Server {}

pub enum Request {
    TcpConnect(TcpStream, Address),
}

impl Server {
    pub async fn listen<T, S>(addr: SocketAddr, ombrac: Client<T>) -> Result<(), Box<dyn Error>>
    where
        T: Provider<Item = S> + Send + Sync + 'static,
        S: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
    {
        use ombrac::io::util::copy_bidirectional;

        let ombrac = Arc::new(ombrac);
        let listener = TcpListener::bind(addr).await?;

        while let Ok((stream, _addr)) = listener.accept().await {
            let ombrac = ombrac.clone();

            tokio::spawn(async move {
                let request = try_or_return!(Self::handler_v5(stream).await);

                match request {
                    Request::TcpConnect(mut inbound, address) => {
                        let mut outbound =
                            try_or_return!(ombrac.tcp_connect(address.clone()).await);

                        let bytes =
                            try_or_return!(copy_bidirectional(&mut inbound, &mut outbound).await);

                        info!(
                            "TcpConnect {:?} send {}, receive {}",
                            address, bytes.0, bytes.1
                        );
                    }
                };
            });
        }

        Ok(())
    }
}
