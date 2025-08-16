use std::io;
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};

use bytes::{Buf, BufMut, Bytes, BytesMut};

/// # ADDRESS
///
/// ```text
/// +------------+------------------------+------------------------+
/// |  ATYP (1)  |       ADDR (*)         |       PORT (2)         |
/// +------------+------------------------+------------------------+
/// ```
///
/// ## ATYP
///
/// ```text
/// +------+--------------+
/// | Bits |    ATYP      |
/// +======+==============+
/// | 0x01 |    Domain    |
/// +------+--------------+
/// | 0x02 |    IPv4      |
/// +------+--------------+
/// | 0x03 |    IPv6      |
/// +------+--------------+
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum Address {
    Domain(Domain, u16),
    IPv4(SocketAddrV4),
    IPv6(SocketAddrV6),
}

impl Address {
    const ADDRESS_ATYP_DOMAIN: u8 = 0x01;
    const ADDRESS_ATYP_IPV4: u8 = 0x02;
    const ADDRESS_ATYP_IPV6: u8 = 0x03;

    const PORT_LENGTH: usize = 2;
    const IPV4_LENGTH: usize = 4;
    const IPV6_LENGTH: usize = 16;

    #[inline]
    pub fn to_bytes(&self) -> io::Result<Bytes> {
        let mut buf = BytesMut::new();

        match self {
            Address::Domain(domain, port) => {
                buf.put_u8(Self::ADDRESS_ATYP_DOMAIN);
                buf.put_u8(domain.len() as u8);
                buf.put_slice(domain.as_bytes());
                buf.put_u16(*port);
            }
            Address::IPv4(addr) => {
                buf.put_u8(Self::ADDRESS_ATYP_IPV4);
                buf.put_slice(&addr.ip().octets());
                buf.put_u16(addr.port());
            }
            Address::IPv6(addr) => {
                buf.put_u8(Self::ADDRESS_ATYP_IPV6);
                buf.put_slice(&addr.ip().octets());
                buf.put_u16(addr.port());
            }
        }

        Ok(buf.freeze())
    }

    #[inline]
    pub fn from_bytes<B: Buf>(buf: &mut B) -> io::Result<Self> {
        if buf.remaining() < 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Insufficient data for address type",
            ));
        }

        let atyp = buf.get_u8();

        match atyp {
            Self::ADDRESS_ATYP_DOMAIN => {
                if buf.remaining() < 1 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Insufficient data for domain length",
                    ));
                }

                let len = buf.get_u8() as usize;
                if buf.remaining() < len + Self::PORT_LENGTH {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Insufficient data for domain and port",
                    ));
                }

                let domain_bytes = buf.copy_to_bytes(len);
                let port = buf.get_u16();
                let domain = Domain::from_bytes(domain_bytes)?;

                Ok(Address::Domain(domain, port))
            }

            Self::ADDRESS_ATYP_IPV4 => {
                if buf.remaining() < Self::IPV4_LENGTH + Self::PORT_LENGTH {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Insufficient data for IPv4 address and port",
                    ));
                }

                let mut ip_bytes = [0u8; 4];
                buf.copy_to_slice(&mut ip_bytes);
                let port = buf.get_u16();

                let addr = SocketAddrV4::new(ip_bytes.into(), port);
                Ok(Address::IPv4(addr))
            }

            Self::ADDRESS_ATYP_IPV6 => {
                if buf.remaining() < Self::IPV6_LENGTH + Self::PORT_LENGTH {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Insufficient data for IPv6 address and port",
                    ));
                }

                let mut ip_bytes = [0u8; 16];
                buf.copy_to_slice(&mut ip_bytes);
                let port = buf.get_u16();

                let addr = SocketAddrV6::new(ip_bytes.into(), port, 0, 0);
                Ok(Address::IPv6(addr))
            }

            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid address type value: {atyp}"),
            )),
        }
    }

    pub async fn to_socket_addr(self) -> io::Result<SocketAddr> {
        use tokio::net::lookup_host;

        match self {
            Address::IPv4(addr) => Ok(SocketAddr::V4(addr)),
            Address::IPv6(addr) => Ok(SocketAddr::V6(addr)),
            Address::Domain(domain, port) => {
                let domain = domain.format_as_str();

                lookup_host((domain, port))
                    .await?
                    .next()
                    .ok_or(io::Error::other(format!(
                        "Failed to resolve domain {domain}"
                    )))
            }
        }
    }
}

impl From<SocketAddr> for Address {
    #[inline]
    fn from(value: SocketAddr) -> Self {
        match value {
            SocketAddr::V4(addr) => Self::IPv4(addr),
            SocketAddr::V6(addr) => Self::IPv6(addr),
        }
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::Domain(domain, port) => format!("{}:{}", domain.format_as_str(), port),
            Self::IPv4(addr) => addr.to_string(),
            Self::IPv6(addr) => addr.to_string(),
        };

        write!(f, "{value}")
    }
}

impl TryFrom<&str> for Address {
    type Error = io::Error;

    #[inline]
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        use std::str::FromStr;

        if let Ok(ipv4_addr) = SocketAddrV4::from_str(value) {
            return Ok(Address::IPv4(ipv4_addr));
        }

        if let Ok(addr) = SocketAddrV6::from_str(value) {
            return Ok(Address::IPv6(addr));
        }

        if let Some((domain, port_str)) = value.rsplit_once(':') {
            if let Ok(port) = port_str.parse::<u16>() {
                if !domain.is_empty() {
                    return Ok(Address::Domain(Domain::try_from(domain)?, port));
                }
            }
        }

        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Invalid address format: {value}"),
        ))
    }
}

impl TryFrom<String> for Address {
    type Error = io::Error;

    #[inline]
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Address::try_from(value.as_str())
    }
}

// ===== Domain =====
#[derive(Debug, Clone, PartialEq)]
pub struct Domain(Bytes);

impl Domain {
    const MAX_LENGTH: usize = 254;

    #[inline]
    pub fn from_bytes(bytes: Bytes) -> io::Result<Self> {
        if bytes.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Domain is empty",
            ));
        }

        let domain_str = std::str::from_utf8(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        if domain_str.len() > Self::MAX_LENGTH {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Punycode domain exceeds maximum length",
            ));
        }

        Ok(Self(bytes))
    }

    #[inline]
    pub fn from_string(value: String) -> io::Result<Self> {
        Self::from_bytes(value.into())
    }

    #[inline]
    pub fn format_as_str(&self) -> &str {
        use std::str::from_utf8;

        from_utf8(&self.0).expect("Invalid UTF-8")
    }

    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    #[inline]
    pub fn to_bytes(self) -> Bytes {
        self.0
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl TryFrom<&[u8]> for Domain {
    type Error = io::Error;

    #[inline]
    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        Self::from_bytes(Bytes::copy_from_slice(value))
    }
}

impl TryFrom<&str> for Domain {
    type Error = io::Error;

    #[inline]
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::from_bytes(Bytes::copy_from_slice(value.as_bytes()))
    }
}

impl TryFrom<String> for Domain {
    type Error = io::Error;

    #[inline]
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::from_string(value)
    }
}

impl TryFrom<Bytes> for Domain {
    type Error = io::Error;

    #[inline]
    fn try_from(value: Bytes) -> Result<Self, Self::Error> {
        Self::from_bytes(value)
    }
}

impl AsRef<[u8]> for Domain {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn test_ipv4_serialization() {
        let addr = Address::IPv4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 8080));
        let bytes = addr.to_bytes().unwrap();
        let mut buf = &bytes[..];
        let parsed = Address::from_bytes(&mut buf).unwrap();
        assert_eq!(addr, parsed);
    }

    #[test]
    fn test_ipv6_serialization() {
        let addr = Address::IPv6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, 8080, 0, 0));
        let bytes = addr.to_bytes().unwrap();
        let mut buf = &bytes[..];
        let parsed = Address::from_bytes(&mut buf).unwrap();
        assert_eq!(addr, parsed);
    }

    #[test]
    fn test_domain_serialization() {
        let domain = Domain::try_from("example.com").unwrap();
        let addr = Address::Domain(domain, 8080);
        let bytes = addr.to_bytes().unwrap();
        let mut buf = &bytes[..];

        let parsed = Address::from_bytes(&mut buf).unwrap();

        if let Address::Domain(d, p) = parsed {
            assert_eq!(d.format_as_str(), "example.com");
            assert_eq!(p, 8080);
        } else {
            panic!("Parsed address is not Domain type");
        }
    }

    #[test]
    fn test_invalid_atyp() {
        let mut buf = bytes::BytesMut::new();
        buf.put_u8(0x04);
        let mut buf = buf.freeze();
        let result = Address::from_bytes(&mut buf);
        assert!(result.is_err());
    }

    #[test]
    fn test_domain_too_long() {
        let result = Domain::try_from(vec![b'a'; 255].as_slice());
        assert!(result.is_err())
    }

    #[tokio::test]
    async fn test_domain_resolution() {
        let domain = Domain::try_from("localhost").unwrap();
        let addr = Address::Domain(domain, 8080);
        let socket_addr = addr.to_socket_addr().await.unwrap();
        assert!(socket_addr.port() == 8080);
    }

    #[test]
    fn test_domain_utf8_error() {
        let result = Domain::from_bytes(Bytes::copy_from_slice(vec![0xff, 0xfe].as_slice()));
        assert!(result.is_err())
    }

    #[test]
    fn test_socket_addr_conversion() {
        let socket_v4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 8080);
        let addr: Address = SocketAddr::V4(socket_v4).into();
        assert!(matches!(addr, Address::IPv4(_)));

        let socket_v6 = SocketAddrV6::new(Ipv6Addr::LOCALHOST, 8080, 0, 0);
        let addr: Address = SocketAddr::V6(socket_v6).into();
        assert!(matches!(addr, Address::IPv6(_)));
    }
}
