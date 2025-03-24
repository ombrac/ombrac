use std::io;

use bytes::{Buf, BufMut, Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::address::Address;
use crate::{SECRET_LENGTH, Secret};

/// # Connect
///
/// ```text
/// +------------------------+------------------------------------+
/// |       LENGTH (2)       |            SECRET (32)             |
/// +========================+====================================+
/// |                         ADDRESS (*)                         |
/// +-------------------------------------------------------------+
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Connect {
    pub secret: Secret,
    pub address: Address,
}

impl Connect {
    pub fn with<A: Into<Address>>(secret: Secret, addr: A) -> Self {
        Self {
            secret,
            address: addr.into(),
        }
    }

    #[inline]
    pub fn to_bytes(&self) -> io::Result<Bytes> {
        let mut buf = BytesMut::new();

        let address = self.address.to_bytes()?;

        buf.put_u16(address.len() as u16);
        buf.put_slice(&self.secret);
        buf.put_slice(&address);

        Ok(buf.freeze())
    }

    #[inline]
    pub fn from_bytes<B: Buf>(buf: &mut B) -> io::Result<Self> {
        if buf.remaining() < 2 + SECRET_LENGTH {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Insufficient data for header",
            ));
        }

        let payload_len = buf.get_u16() as usize;

        let mut secret = [0u8; SECRET_LENGTH];
        buf.copy_to_slice(&mut secret);

        if buf.remaining() < payload_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Insufficient data for address",
            ));
        }

        let mut address_bytes = buf.copy_to_bytes(payload_len);
        let address = Address::from_bytes(&mut address_bytes)?;

        Ok(Self { secret, address })
    }

    #[inline]
    pub async fn from_async_read<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Self> {
        let payload_len = reader.read_u16().await?;

        let mut buf = vec![0u8; SECRET_LENGTH + payload_len as usize];
        reader.read_exact(&mut buf).await?;

        let mut secret = [0u8; SECRET_LENGTH];
        secret.copy_from_slice(&buf[..SECRET_LENGTH]);

        let address = Address::from_bytes(&mut &buf[SECRET_LENGTH..])?;

        Ok(Self { secret, address })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use bytes::BytesMut;
    use std::net::{Ipv4Addr, SocketAddrV4};

    use crate::address::Domain;

    #[test]
    fn test_connect_serialization() {
        let secret = [0xAA; 32];
        let address = Address::IPv4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 8080));

        let connect = Connect::with(secret, address.clone());

        let bytes = connect.to_bytes().unwrap();

        let mut buf = &bytes[..];
        let parsed = Connect::from_bytes(&mut buf).unwrap();

        assert_eq!(parsed.secret, secret);
        assert_eq!(parsed.address, address);
    }

    #[test]
    fn test_connect_with_domain_address() {
        let secret = [0xBB; 32];
        let domain = "example.com";
        let address = Address::Domain(Domain::from(domain), 8080);

        let connect = Connect::with(secret, address);

        let bytes = connect.to_bytes().unwrap();
        let mut buf = &bytes[..];
        let parsed = Connect::from_bytes(&mut buf).unwrap();

        assert_eq!(parsed.secret, secret);
        if let Address::Domain(d, p) = parsed.address {
            assert_eq!(d.format_as_str().unwrap(), domain);
            assert_eq!(p, 8080);
        } else {
            panic!("Parsed address is not Domain type");
        }
    }

    #[test]
    fn test_connect_insufficient_header() {
        let mut buf = BytesMut::new();
        buf.put_u8(0x00);

        let result = Connect::from_bytes(&mut buf.freeze());
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn test_connect_insufficient_payload() {
        let mut buf = BytesMut::new();
        buf.put_u16(100);
        buf.put_slice(&[0xDD; 32]);

        let result = Connect::from_bytes(&mut buf.freeze());
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn test_connect_invalid_address() {
        let mut buf = BytesMut::new();
        buf.put_u16(10);
        buf.put_slice(&[0xEE; 32]);
        buf.put_u8(0x04);

        let result = Connect::from_bytes(&mut buf.freeze());
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn test_connect_from_async_read() {
        let secret = [0xFF; 32];
        let address = Address::IPv4(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 1), 80));

        let connect = Connect::with(secret, address.clone());

        let bytes = connect.to_bytes().unwrap();

        let mut reader = &bytes[..];
        let parsed = Connect::from_async_read(&mut reader).await.unwrap();

        assert_eq!(parsed.secret, secret);
        assert_eq!(parsed.address, address);
    }

    #[tokio::test]
    async fn test_connect_from_async_read_with_domain() {
        let secret = [0u8; 32];
        let address = Address::Domain(Domain::from("example.com"), 443);

        let connect = Connect::with(secret, address.clone());

        let bytes = connect.to_bytes().unwrap();

        let mut reader = &bytes[..];
        let parsed = Connect::from_async_read(&mut reader).await.unwrap();

        assert_eq!(parsed.secret, secret);
        assert_eq!(parsed.address, address);
    }

    #[tokio::test]
    async fn test_connect_from_async_read_insufficient_data() {
        let mut reader = &[0x00; 10][..];

        let result = Connect::from_async_read(&mut reader).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    async fn test_connect_from_async_read_invalid_address() {
        let mut buf = BytesMut::new();
        buf.put_u16(10);
        buf.put_slice(&[0xEE; 32]);
        buf.put_u8(0x04);
        buf.put_slice(&[0x00; 9]);

        let mut reader = &buf.freeze()[..];
        let result = Connect::from_async_read(&mut reader).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
}
