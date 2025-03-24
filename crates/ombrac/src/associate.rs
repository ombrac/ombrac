use std::io;

use bytes::{Buf, BufMut, Bytes, BytesMut};

use crate::address::Address;
use crate::{SECRET_LENGTH, Secret};

/// # Associate
///
/// ```text
/// +------------------------+------------------------------------+
/// |       LENGTH (2)       |            SECRET (32)             |
/// +========================+====================================+
/// |                         ADDRESS (*)                         |
/// +-------------------------------------------------------------+
/// |                           DATA (*)                          |
/// +-------------------------------------------------------------+
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Associate {
    pub secret: Secret,
    pub address: Address,
    pub data: Bytes,
}

impl Associate {
    pub fn with<A, B>(secret: Secret, addr: A, data: B) -> Self
    where
        A: Into<Address>,
        B: Into<Bytes>,
    {
        Self {
            secret,
            address: addr.into(),
            data: data.into(),
        }
    }

    #[inline]
    pub fn to_bytes(&self) -> io::Result<Bytes> {
        let mut buf = BytesMut::new();

        let address_bytes = self.address.to_bytes()?;
        let data_bytes = &self.data;

        let payload_len = address_bytes.len() + data_bytes.len();

        if payload_len > u16::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "Payload length {} exceeds maximum allowed length {}",
                    payload_len,
                    u16::MAX
                ),
            ));
        }

        buf.put_u16(payload_len as u16);

        buf.put_slice(&self.secret);
        buf.put_slice(&address_bytes);
        buf.put_slice(data_bytes);

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

        let mut secret = Secret::default();
        buf.copy_to_slice(&mut secret);

        if buf.remaining() < payload_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Insufficient data for address and data",
            ));
        }

        let address = Address::from_bytes(buf)?;
        let data = buf.copy_to_bytes(buf.remaining());

        Ok(Self {
            secret,
            address,
            data,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use bytes::{Bytes, BytesMut};
    use std::net::{Ipv4Addr, SocketAddrV4};

    use crate::address::Domain;

    #[test]
    fn test_associate_serialization() {
        let secret = [0xAA; 32];
        let address = Address::IPv4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 8080));
        let data = Bytes::from("test data");

        let associate = Associate::with(secret, address.clone(), data.clone());

        let bytes = associate.to_bytes().unwrap();

        let mut buf = &bytes[..];
        let parsed = Associate::from_bytes(&mut buf).unwrap();

        assert_eq!(parsed.secret, secret);
        assert_eq!(parsed.address, address);
        assert_eq!(parsed.data, data);
    }

    #[test]
    fn test_associate_with_domain_address() {
        let secret = [0xBB; 32];
        let domain = "example.com";
        let address = Address::Domain(Domain::from(domain), 8080);
        let data = Bytes::from("domain test data");

        let associate = Associate::with(secret, address, data.clone());

        let bytes = associate.to_bytes().unwrap();
        let mut buf = &bytes[..];
        let parsed = Associate::from_bytes(&mut buf).unwrap();

        assert_eq!(parsed.secret, secret);
        if let Address::Domain(d, p) = parsed.address {
            assert_eq!(d.format_as_str().unwrap(), domain);
            assert_eq!(p, 8080);
        } else {
            panic!("Parsed address is not Domain type");
        }
        assert_eq!(parsed.data, data);
    }

    #[test]
    fn test_associate_empty_data() {
        let secret = [0xCC; 32];
        let address = Address::IPv4(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 1), 80));
        let data = Bytes::new();

        let associate = Associate::with(secret, address.clone(), data.clone());

        let bytes = associate.to_bytes().unwrap();
        let mut buf = &bytes[..];
        let parsed = Associate::from_bytes(&mut buf).unwrap();

        assert_eq!(parsed.secret, secret);
        assert_eq!(parsed.address, address);
        assert_eq!(parsed.data, data);
    }

    #[test]
    fn test_associate_insufficient_header() {
        let mut buf = BytesMut::new();
        buf.put_u8(0x00);

        let result = Associate::from_bytes(&mut buf.freeze());
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn test_associate_insufficient_payload() {
        let mut buf = BytesMut::new();
        buf.put_u16(100);
        buf.put_slice(&[0xDD; 32]);

        let result = Associate::from_bytes(&mut buf.freeze());
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn test_associate_invalid_address() {
        let mut buf = BytesMut::new();
        buf.put_u16(10);
        buf.put_slice(&[0xEE; 32]);
        buf.put_u8(0x04);

        let result = Associate::from_bytes(&mut buf.freeze());
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn test_associate_large_data() {
        let secret = [0xFF; 32];
        let address = Address::IPv4(SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 1), 443));
        let data = Bytes::from(vec![0x11; 65528]);

        let associate = Associate::with(secret, address.clone(), data.clone());

        let bytes = associate.to_bytes().unwrap();
        let mut buf = &bytes[..];
        let parsed = Associate::from_bytes(&mut buf).unwrap();

        assert_eq!(parsed.secret, secret);
        assert_eq!(parsed.address, address);
        assert_eq!(parsed.data, data);
    }

    #[test]
    fn test_to_bytes_exceed_max() {
        let secret = [0xCC; 32];
        let address = Address::IPv4(SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 1), 443));
        let data = Bytes::from(vec![0x22; u16::MAX as usize]);

        let associate = Associate::with(secret, address, data);
        let result = associate.to_bytes();

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }
}
