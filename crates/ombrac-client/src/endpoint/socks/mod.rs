mod v5;

use std::error::Error;
use std::net::SocketAddr;

use ombrac::request::Address;
use ombrac::Provider;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};

use crate::Client;
use crate::{error, info};

pub struct Server {}

pub enum Request {
    TcpConnect(TcpStream, Address),
}

impl Server {
    pub async fn listen<T, S>(addr: SocketAddr, mut ombrac: Client<T>) -> Result<(), Box<dyn Error>>
    where
        T: Provider<Item = S> + Send + 'static,
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let listener = TcpListener::bind(addr).await?;

        while let Ok((stream, _addr)) = listener.accept().await {
            let mut outbound = match ombrac.outbound().await {
                Ok(value) => value,
                Err(_error) => {
                    error!("{_error}");

                    continue;
                }
            };

            tokio::spawn(async move {
                let request = match Self::handler_v5(stream).await {
                    Ok(value) => value,
                    Err(_error) => {
                        error!("{_error}");

                        return;
                    }
                };

                match request {
                    Request::TcpConnect(mut inbound, address) => {
                        if let Err(_error) =
                            Client::<T>::tcp_connect(&mut outbound, address.clone()).await
                        {
                            error!("{_error}");

                            return;
                        };

                        match ombrac::io::util::copy_bidirectional(&mut inbound, &mut outbound)
                            .await
                        {
                            Ok(value) => {
                                info!(
                                    "TcpConnect {:?} send {}, receive {}",
                                    address, value.0, value.1
                                );
                            }

                            Err(_error) => {
                                error!("TcpConnect {:?} error, {}", address, _error);

                                return;
                            }
                        }
                    }
                };
            });
        }

        Ok(())
    }
}
