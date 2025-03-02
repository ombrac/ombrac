use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

use bytes::{Buf, BufMut, Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt};

const ADDRESS_ATYP_DOMAIN: u8 = 1;
const ADDRESS_ATYP_IPV4: u8 = 2;
const ADDRESS_ATYP_IPV6: u8 = 3;

const IPV4_LENGTH: usize = 4;
const IPV6_LENGTH: usize = 16;

//  +------------+------------------------+------------------------+
//  |  ATYP (8)  |       PORT (16)        |      ADDR (0...)     ...
//  +------------+------------------------+------------------------+
#[derive(Debug, Clone)]
pub enum Address {
    Domain(String, u16),
    IPv4(SocketAddrV4),
    IPv6(SocketAddrV6),
}

impl Address {
    pub async fn to_socket_addr(self) -> io::Result<SocketAddr> {
        use tokio::net::lookup_host;

        match self {
            Address::IPv4(addr) => Ok(SocketAddr::V4(addr)),
            Address::IPv6(addr) => Ok(SocketAddr::V6(addr)),
            Address::Domain(domain, port) => lookup_host((domain.as_str(), port))
                .await?
                .next()
                .ok_or(io::Error::other(format!(
                    "Failed to resolve domain {}",
                    domain
                ))),
        }
    }

    pub fn to_bytes(&self) -> io::Result<Bytes> {
        let mut buf = BytesMut::new();

        match self {
            Address::Domain(domain, port) => {
                let domain_bytes = domain.as_bytes();
                let domain_len = domain_bytes.len();
                if domain_len > 255 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Domain name exceeds maximum length of 255 bytes",
                    ));
                }
                buf.reserve(1 + 2 + 1 + domain_len);
                buf.put_u8(ADDRESS_ATYP_DOMAIN);
                buf.put_u16(*port);
                buf.put_u8(domain_len as u8);
                buf.put_slice(domain_bytes);
            }
            Address::IPv4(addr) => {
                buf.reserve(1 + 2 + IPV4_LENGTH);
                buf.put_u8(ADDRESS_ATYP_IPV4);
                buf.put_u16(addr.port());
                buf.put_slice(&addr.ip().octets());
            }
            Address::IPv6(addr) => {
                buf.reserve(1 + 2 + IPV6_LENGTH);
                buf.put_u8(ADDRESS_ATYP_IPV6);
                buf.put_u16(addr.port());
                buf.put_slice(&addr.ip().octets());
            }
        }

        Ok(buf.freeze())
    }

    pub fn from_bytes<B: Buf>(buf: &mut B) -> io::Result<Self> {
        if buf.remaining() < 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Insufficient data for address type",
            ));
        }

        let atyp = buf.get_u8();

        if buf.remaining() < 2 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Insufficient data for port",
            ));
        }

        let port = buf.get_u16();

        match atyp {
            ADDRESS_ATYP_DOMAIN => {
                if buf.remaining() < 1 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Insufficient data for domain length",
                    ));
                }

                let len = buf.get_u8() as usize;

                if buf.remaining() < len {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Insufficient data for domain",
                    ));
                }

                let domain_bytes = buf.copy_to_bytes(len);
                let domain = String::from_utf8(domain_bytes.to_vec()).map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("Invalid domain: {}", e))
                })?;

                Ok(Address::Domain(domain, port))
            }
            ADDRESS_ATYP_IPV4 => {
                if buf.remaining() < IPV4_LENGTH {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Insufficient data for IPv4",
                    ));
                }

                let mut ip = [0u8; IPV4_LENGTH];
                buf.copy_to_slice(&mut ip);

                Ok(Address::IPv4(SocketAddrV4::new(Ipv4Addr::from(ip), port)))
            }
            ADDRESS_ATYP_IPV6 => {
                if buf.remaining() < IPV6_LENGTH {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Insufficient data for IPv6",
                    ));
                }

                let mut ip = [0u8; IPV6_LENGTH];
                buf.copy_to_slice(&mut ip);

                Ok(Address::IPv6(SocketAddrV6::new(
                    Ipv6Addr::from(ip),
                    port,
                    0,
                    0,
                )))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid address type value: {}", atyp),
            )),
        }
    }

    pub async fn from_async_read<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Self> {
        let mut header = [0u8; 3];
        reader.read_exact(&mut header).await?;

        let atyp = header[0];
        let port = u16::from_be_bytes([header[1], header[2]]);

        match atyp {
            ADDRESS_ATYP_DOMAIN => {
                let mut len = [0u8; 1];
                reader.read_exact(&mut len).await?;
                let len = len[0] as usize;

                let mut domain = vec![0u8; len];
                reader.read_exact(&mut domain).await?;
                let domain = String::from_utf8(domain).map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("Invalid domain: {}", e))
                })?;

                Ok(Address::Domain(domain, port))
            }
            ADDRESS_ATYP_IPV4 => {
                let mut ip = [0u8; IPV4_LENGTH];
                reader.read_exact(&mut ip).await?;

                Ok(Address::IPv4(SocketAddrV4::new(Ipv4Addr::from(ip), port)))
            }
            ADDRESS_ATYP_IPV6 => {
                let mut ip = [0u8; IPV6_LENGTH];
                reader.read_exact(&mut ip).await?;

                Ok(Address::IPv6(SocketAddrV6::new(
                    Ipv6Addr::from(ip),
                    port,
                    0,
                    0,
                )))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid address type value: {}", atyp),
            )),
        }
    }
}

impl Into<Address> for SocketAddr {
    fn into(self) -> Address {
        match self {
            Self::V4(addr) => Address::IPv4(addr),
            Self::V6(addr) => Address::IPv6(addr),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_address_domain_serialization() {
        let domain_addr = Address::Domain("example.com".to_string(), 8080);
        let bytes = domain_addr.to_bytes().unwrap();
        let mut buf = bytes.as_ref();
        let decoded = Address::from_bytes(&mut buf).unwrap();

        match decoded {
            Address::Domain(domain, port) => {
                assert_eq!(domain, "example.com");
                assert_eq!(port, 8080);
            }
            _ => panic!("Wrong address type"),
        }
    }

    #[test]
    fn test_address_ipv4_serialization() {
        let ipv4_addr = Address::IPv4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 8080));
        let bytes = ipv4_addr.to_bytes().unwrap();
        let mut buf = bytes.as_ref();
        let decoded = Address::from_bytes(&mut buf).unwrap();

        match decoded {
            Address::IPv4(addr) => {
                assert_eq!(addr.ip().octets(), [127, 0, 0, 1]);
                assert_eq!(addr.port(), 8080);
            }
            _ => panic!("Wrong address type"),
        }
    }

    #[test]
    fn test_address_ipv6_serialization() {
        let ipv6_addr = Address::IPv6(SocketAddrV6::new(
            Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1),
            8080,
            0,
            0,
        ));
        let bytes = ipv6_addr.to_bytes().unwrap();
        let mut buf = bytes.as_ref();
        let decoded = Address::from_bytes(&mut buf).unwrap();

        match decoded {
            Address::IPv6(addr) => {
                assert_eq!(addr.ip().segments(), [0x2001, 0xdb8, 0, 0, 0, 0, 0, 1]);
                assert_eq!(addr.port(), 8080);
            }
            _ => panic!("Wrong address type"),
        }
    }

    #[test]
    fn test_address_domain_too_long() {
        let long_domain = "a".repeat(256);
        let addr = Address::Domain(long_domain, 8080);
        assert!(addr.to_bytes().is_err());
    }

    #[test]
    fn test_address_from_bytes_insufficient_data() {
        // Test with empty buffer
        let mut empty_buf = Bytes::new();
        assert!(Address::from_bytes(&mut empty_buf).is_err());

        // Test with only address type
        let mut small_buf = BytesMut::new();
        small_buf.put_u8(ADDRESS_ATYP_DOMAIN);
        assert!(Address::from_bytes(&mut small_buf.freeze()).is_err());
    }

    #[test]
    fn test_address_invalid_type() {
        let mut buf = BytesMut::new();
        buf.put_u8(255); // Invalid address type
        buf.put_u16(8080);
        assert!(Address::from_bytes(&mut buf.freeze()).is_err());
    }

    #[test]
    fn test_address_invalid_domain() {
        let mut buf = BytesMut::new();
        buf.put_u8(ADDRESS_ATYP_DOMAIN);
        buf.put_u16(8080);
        buf.put_u8(4); // domain length
        buf.put_slice(&[0xFF, 0xFF, 0xFF, 0xFF]); // Invalid UTF-8
        assert!(Address::from_bytes(&mut buf.freeze()).is_err());
    }
}

#[cfg(test)]
mod tests_async {
    use bytes::BytesMut;
    use std::net::{Ipv4Addr, Ipv6Addr};

    use super::*;

    async fn write_to_buf(data: &[u8]) -> impl AsyncRead + Unpin {
        use std::io::Cursor;

        Cursor::new(Vec::from(data))
    }

    #[tokio::test]
    async fn test_from_async_read_domain() {
        let mut data = BytesMut::new();
        data.put_u8(ADDRESS_ATYP_DOMAIN);
        data.put_u16(8080);
        data.put_u8(11);
        data.put_slice(b"example.com");

        let mut reader = write_to_buf(&data).await;

        let address = Address::from_async_read(&mut reader).await.unwrap();

        match address {
            Address::Domain(domain, port) => {
                assert_eq!(domain, "example.com");
                assert_eq!(port, 8080);
            }
            _ => panic!("Expected Address::Domain"),
        }
    }

    #[tokio::test]
    async fn test_from_async_read_ipv4() {
        let mut data = BytesMut::new();
        data.put_u8(ADDRESS_ATYP_IPV4);
        data.put_u16(8080);
        data.put_slice(&[127, 0, 0, 1]);

        let mut reader = write_to_buf(&data).await;

        let address = Address::from_async_read(&mut reader).await.unwrap();

        match address {
            Address::IPv4(addr) => {
                assert_eq!(addr.ip(), &Ipv4Addr::new(127, 0, 0, 1));
                assert_eq!(addr.port(), 8080);
            }
            _ => panic!("Expected Address::IPv4"),
        }
    }

    #[tokio::test]
    async fn test_from_async_read_ipv6() {
        let mut data = BytesMut::new();
        data.put_u8(ADDRESS_ATYP_IPV6);
        data.put_u16(8080);
        data.put_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);

        let mut reader = write_to_buf(&data).await;

        let address = Address::from_async_read(&mut reader).await.unwrap();

        match address {
            Address::IPv6(addr) => {
                assert_eq!(addr.ip(), &Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1));
                assert_eq!(addr.port(), 8080);
            }
            _ => panic!("Expected Address::IPv6"),
        }
    }

    #[tokio::test]
    async fn test_from_async_read_invalid_atyp() {
        let mut data = BytesMut::new();
        data.put_u8(4);
        data.put_u16(8080);

        let mut reader = write_to_buf(&data).await;

        let result = Address::from_async_read(&mut reader).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn test_from_async_read_incomplete_data() {
        let mut data = BytesMut::new();
        data.put_u8(ADDRESS_ATYP_IPV4);

        let mut reader = write_to_buf(&data).await;

        let result = Address::from_async_read(&mut reader).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    async fn test_from_async_read_invalid_domain() {
        let mut data = BytesMut::new();
        data.put_u8(ADDRESS_ATYP_DOMAIN);
        data.put_u16(8080);
        data.put_u8(3);
        data.put_slice(&[0xFF, 0xFF, 0xFF]);

        let mut reader = write_to_buf(&data).await;

        let result = Address::from_async_read(&mut reader).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
}
