mod v5;

use std::error::Error;
use std::marker::PhantomData;

use ombrac::io::IntoSplit;
use ombrac::request::Address;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::{error, info};

pub struct Config {
    listen: String,
}

pub struct Server<Client, Stream> {
    config: Config,
    client: Client,
    _stream: PhantomData<Stream>,
}

pub enum Request {
    TcpConnect(TcpStream, Address),
}

impl Config {
    pub fn new(addr: String) -> Self {
        Self { listen: addr }
    }
}

impl<Client, Stream> Server<Client, Stream> {
    pub fn new(config: Config, client: Client) -> Self {
        Self {
            config,
            client,
            _stream: PhantomData,
        }
    }
}

impl<Client, Stream> Server<Client, Stream>
where
    Client: ombrac::Client<Stream>,
    Stream: IntoSplit + AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
{
    pub async fn listen(mut self) -> Result<(), Box<dyn Error>> {
        let listener = TcpListener::bind(self.config.listen).await?;

        loop {
            let outbound = match self.client.outbound().await {
                Some(value) => value,
                None => return Ok(()),
            };

            match listener.accept().await {
                Ok((stream, _)) => {
                    tokio::spawn(async move {
                        let request = match Self::handler_v5(stream).await {
                            Ok(value) => value,
                            Err(_error) => {
                                error!("{_error}");

                                return;
                            }
                        };

                        match request {
                            Request::TcpConnect(inbound, address) => {
                                info!("TcpConnect {:?}", address);

                                if let Err(_error) =
                                    Client::warp_tcp(inbound, outbound, address).await
                                {
                                    error!("{_error}")
                                }
                            }
                        }
                    });
                }

                Err(_error) => {
                    error!("socks server failed to accept: {:?}", _error);
                }
            };
        }
    }
}
