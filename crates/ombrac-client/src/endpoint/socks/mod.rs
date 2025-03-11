mod v5;

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use std::{error::Error, io::Cursor};

use ombrac::prelude::*;
use ombrac_macros::{error, info, try_or_return};
use ombrac_transport::Initiator;
use socks_lib::socks5::Address as Socks5Address;
use socks_lib::ToBytes;
use socks_lib::{socks5::UdpPacket, Streamable};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::time::timeout;

use crate::Client;

pub struct Server {}

pub enum Request {
    TcpConnect(TcpStream, Address),

    #[cfg(feature = "datagram")]
    UdpAssociate(TcpStream, UdpSocket),
}

impl Server {
    pub async fn listen<T>(addr: SocketAddr, ombrac: Client<T>) -> Result<(), Box<dyn Error>>
    where
        T: Initiator + Send + Sync + 'static,
    {
        use ombrac::io::util::copy_bidirectional;

        let ombrac = Arc::new(ombrac);
        let listener = TcpListener::bind(addr).await?;

        info!("SOCKS server listening on {}", listener.local_addr()?);

        while let Ok((stream, _addr)) = listener.accept().await {
            let ombrac = ombrac.clone();

            tokio::spawn(async move {
                let request = try_or_return!(Self::handler_v5(stream).await);

                match request {
                    Request::TcpConnect(mut inbound, addr) => {
                        let outbound_result =
                            timeout(Duration::from_secs(30), ombrac.tcp_connect(addr.clone()))
                                .await;

                        let mut outbound = match outbound_result {
                            Ok(Ok(stream)) => stream,
                            Ok(Err(_error)) => {
                                error!("Failed to connect to {:?}: {}", addr, _error);
                                return;
                            }
                            Err(_) => {
                                error!("Connection to {:?} timed out", addr);
                                return;
                            }
                        };

                        let _bytes =
                            try_or_return!(copy_bidirectional(&mut inbound, &mut outbound).await);

                        info!(
                            "TCP Connect {:?} Send {}, Receive {}",
                            addr, _bytes.0, _bytes.1
                        );
                    }

                    #[cfg(feature = "datagram")]
                    Request::UdpAssociate(stream, socket) => {
                        let unr = ombrac.udp_associate().await.unwrap();

                        let socks_1 = Arc::new(socket);
                        let socks_2 = socks_1.clone();
                        let datagram_recv = Arc::new(unr);
                        let datagram_send = datagram_recv.clone();

                        let mut buf = [0u8; 2048];

                        let (len, client_socks_addr) = socks_2.recv_from(&mut buf).await.unwrap();
                        info!("Udp Associate from {}, {}", client_socks_addr, len);

                        let data = buf[..len].to_vec();
                        let socks_packet = UdpPacket::read(&mut Cursor::new(data)).await.unwrap();

                        let addr = match socks_packet.address {
                            Socks5Address::Domain(domain, port) => Address::Domain(domain, port),
                            Socks5Address::IPv4(addr) => Address::IPv4(addr),
                            Socks5Address::IPv6(addr) => Address::IPv6(addr),
                        };
                        let data = socks_packet.data;

                        datagram_send.send(addr, data).await.unwrap();

                        let handle = tokio::spawn(async move {
                            loop {
                                let (len, _addr) = socks_2.recv_from(&mut buf).await.unwrap();
                                let data = buf[..len].to_vec();
                                let socks_packet =
                                    UdpPacket::read(&mut Cursor::new(data)).await.unwrap();

                                let addr = match socks_packet.address {
                                    Socks5Address::Domain(domain, port) => {
                                        Address::Domain(domain, port)
                                    }
                                    Socks5Address::IPv4(addr) => Address::IPv4(addr),
                                    Socks5Address::IPv6(addr) => Address::IPv6(addr),
                                };
                                let data = socks_packet.data;

                                if let Err(_error) = datagram_send.send(addr, data).await {
                                    error!("UDP Datagram connection close: {}", _error);

                                    break;
                                }
                            }
                        });

                        let send_handle = tokio::spawn(async move {
                            while let Ok((addr, data)) = datagram_recv.recv().await {
                                info!("UDP recv from remote {:?} {:?}", addr, data.len());
                                let addr = match addr {
                                    Address::Domain(domain, port) => {
                                        Socks5Address::Domain(domain, port)
                                    }
                                    Address::IPv4(addr) => Socks5Address::IPv4(addr),
                                    Address::IPv6(addr) => Socks5Address::IPv6(addr),
                                };
                                let data = UdpPacket::un_frag(addr, data.into());

                                if socks_1
                                    .send_to(&data.to_bytes(), client_socks_addr)
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                        });

                        loop {
                            tokio::time::sleep(Duration::from_secs(10)).await;

                            let mut buf = [0u8; 1];
                            let is_closed = match stream.peek(&mut buf).await {
                                Ok(0) => true,
                                Ok(_) => false,
                                Err(e) if e.kind() == io::ErrorKind::WouldBlock => false,
                                Err(_) => true,
                            };

                            if is_closed {
                                break;
                            }
                        }

                        handle.abort();
                        send_handle.abort();
                    }
                };
            });
        }

        Ok(())
    }
}
