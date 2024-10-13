use std::io::{Error, ErrorKind, Result};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

use bytes::{BufMut, BytesMut};
use tokio::io::AsyncReadExt;

#[cfg(feature = "udp")]
use tokio::sync::mpsc::{Receiver, Sender};

use crate::{Resolver, Streamable, ToBytes};

#[rustfmt::skip]
mod consts {
    pub const REQUEST_TYPE_TCP_CONNECT:         u8 = 0x01;

    #[cfg(feature = "udp")]
    pub const REQUEST_TYPE_UDP_ASSOCIATE:       u8 = 0x02;

    pub const ADDRESS_TYPE_DOMAIN:              u8 = 0x01;
    pub const ADDRESS_TYPE_IPV4:                u8 = 0x02;
    pub const ADDRESS_TYPE_IPV6:                u8 = 0x03;
}

/// # Request
#[derive(Debug)]
pub enum Request {
    /// ## Bytes
    ///
    /// ```text
    ///      +------+------+----------+------+
    ///      | RTYP | ATYP |   ADDR   | PORT |
    ///      +------+------+----------+------+
    ///      |  1   |  1   | Variable |  2   |
    ///      +------+------+----------+------+
    /// ```
    ///
    TcpConnect(Address),

    /// ## Bytes
    ///
    /// ```text
    ///      +------+
    ///      | RTYP |
    ///      +------+
    ///      |  1   |
    ///      +------+
    /// ```
    #[cfg(feature = "udp")]
    UdpAssociate(Option<(Sender<udp::Datagram>, Receiver<udp::Datagram>)>),
}

impl ToBytes for Request {
    fn to_bytes(&self) -> BytesMut {
        let mut bytes = BytesMut::new();

        match self {
            Self::TcpConnect(value) => {
                bytes.put_u8(consts::REQUEST_TYPE_TCP_CONNECT);
                bytes.extend(value.to_bytes());
            }

            #[cfg(feature = "udp")]
            Self::UdpAssociate(_) => {
                bytes.put_u8(consts::REQUEST_TYPE_UDP_ASSOCIATE);
            }
        };

        bytes
    }
}

impl Streamable for Request {
    async fn read<T>(stream: &mut T) -> Result<Self>
    where
        T: AsyncReadExt + Unpin + Send,
    {
        let request_type = stream.read_u8().await?;

        let request = match request_type {
            consts::REQUEST_TYPE_TCP_CONNECT => Request::TcpConnect(Address::read(stream).await?),

            #[cfg(feature = "udp")]
            consts::REQUEST_TYPE_UDP_ASSOCIATE => Request::UdpAssociate(None),

            _ => {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    format!("unsupported request type {}", request_type),
                ))
            }
        };

        Ok(request)
    }
}

#[derive(Debug, Clone)]
pub enum Address {
    Domain(String, u16),
    IPv4(SocketAddrV4),
    IPv6(SocketAddrV6),
}

impl Address {
    pub async fn to_socket_address<R>(self, resolver: &R) -> Result<SocketAddr>
    where
        R: Resolver,
    {
        let socket_address = match self {
            Self::Domain(domain, port) => resolver.lookup(&domain, port).await?.into(),
            Self::IPv4(addr) => addr.into(),
            Self::IPv6(addr) => addr.into(),
        };

        Ok(socket_address)
    }

    pub fn from_socket_address(addr: SocketAddr) -> Self {
        match addr {
            SocketAddr::V4(addr) => Self::IPv4(addr),
            SocketAddr::V6(addr) => Self::IPv6(addr),
        }
    }
}

impl Streamable for Address {
    async fn read<T>(stream: &mut T) -> Result<Self>
    where
        Self: Sized,
        T: AsyncReadExt + Unpin + Send,
    {
        const PORT_LENGTH: usize = 2;
        const IPV4_ADDRESS_LENGTH: usize = 4;
        const IPV6_ADDRESS_LENGTH: usize = 16;

        let address_type = stream.read_u8().await?;

        let address = match address_type {
            consts::ADDRESS_TYPE_DOMAIN => {
                let domain_len = stream.read_u8().await? as usize;

                let mut buffer = vec![0u8; domain_len + 2];
                stream.read_exact(&mut buffer).await?;

                let domain = std::str::from_utf8(&buffer[0..domain_len])
                    .map_err(|_| Error::other("invalid domain name"))?;

                let port = ((buffer[domain_len] as u16) << 8) | (buffer[domain_len + 1] as u16);

                Address::Domain(domain.to_string(), port)
            }

            consts::ADDRESS_TYPE_IPV4 => {
                let mut buffer = [0u8; IPV4_ADDRESS_LENGTH + PORT_LENGTH];
                stream.read_exact(&mut buffer).await?;

                let ip = Ipv4Addr::new(buffer[0], buffer[1], buffer[2], buffer[3]);
                let port = ((buffer[4] as u16) << 8) | (buffer[5] as u16);

                Address::IPv4(SocketAddrV4::new(ip, port))
            }

            consts::ADDRESS_TYPE_IPV6 => {
                let mut buffer = [0u8; IPV6_ADDRESS_LENGTH + PORT_LENGTH];
                stream.read_exact(&mut buffer).await?;

                let ip = Ipv6Addr::new(
                    (buffer[0] as u16) << 8 | buffer[1] as u16,
                    (buffer[2] as u16) << 8 | buffer[3] as u16,
                    (buffer[4] as u16) << 8 | buffer[5] as u16,
                    (buffer[6] as u16) << 8 | buffer[7] as u16,
                    (buffer[8] as u16) << 8 | buffer[9] as u16,
                    (buffer[10] as u16) << 8 | buffer[11] as u16,
                    (buffer[12] as u16) << 8 | buffer[13] as u16,
                    (buffer[14] as u16) << 8 | buffer[15] as u16,
                );
                let port = ((buffer[16] as u16) << 8) | (buffer[17] as u16);

                Address::IPv6(SocketAddrV6::new(ip, port, 0, 0))
            }

            _ => {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    format!("unsupported request address type {}", address_type),
                ))
            }
        };

        Ok(address)
    }
}

impl ToBytes for Address {
    fn to_bytes(&self) -> BytesMut {
        let mut bytes = BytesMut::new();

        match self {
            Self::Domain(domain, port) => {
                let domain_bytes = domain.as_bytes();
                bytes.put_u8(consts::ADDRESS_TYPE_DOMAIN);
                bytes.put_u8(domain_bytes.len() as u8);
                bytes.extend_from_slice(domain_bytes);
                bytes.extend_from_slice(&port.to_be_bytes());
            }
            Self::IPv4(addr) => {
                bytes.put_u8(consts::ADDRESS_TYPE_IPV4);
                bytes.extend_from_slice(&addr.ip().octets());
                bytes.extend_from_slice(&addr.port().to_be_bytes());
            }
            Self::IPv6(addr) => {
                bytes.put_u8(consts::ADDRESS_TYPE_IPV6);
                bytes.extend_from_slice(&addr.ip().octets());
                bytes.extend_from_slice(&addr.port().to_be_bytes());
            }
        }

        bytes
    }
}

#[cfg(feature = "udp")]
pub mod udp {
    use super::*;

    #[derive(Debug, Clone)]
    pub struct Datagram {
        pub address: Address,
        pub length: u16,
        pub data: BytesMut,
    }

    impl ToBytes for Datagram {
        fn to_bytes(&self) -> BytesMut {
            let mut bytes = BytesMut::new();
            bytes.extend_from_slice(&self.address.to_bytes());

            bytes.put_u16(self.length);
            bytes.extend_from_slice(&self.data);

            bytes
        }
    }

    impl Streamable for Datagram {
        async fn read<T>(stream: &mut T) -> Result<Self>
        where
            Self: Sized,
            T: AsyncReadExt + Unpin + Send,
        {
            let address = <Address as Streamable>::read(stream).await?;

            let length = stream.read_u16().await?;

            let mut data = BytesMut::with_capacity(length as usize);
            stream.read_exact(&mut data).await?;

            Ok(Self {
                address,
                length,
                data,
            })
        }
    }

    impl Datagram {
        pub fn with(address: Address, length: u16, data: BytesMut) -> Self {
            Self {
                address,
                length,
                data,
            }
        }

        pub fn address(&self) -> &Address {
            &self.address
        }

        pub fn length(&self) -> u16 {
            self.length
        }

        pub fn data(&self) -> &BytesMut {
            &self.data
        }
    }
}
