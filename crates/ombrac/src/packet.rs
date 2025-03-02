use std::io;

use bytes::{Buf, BufMut, Bytes, BytesMut};

use crate::address::Address;
use crate::{Secret, SECRET_LENGTH};

#[derive(Debug, Clone)]
pub struct Packet {
    pub secret: Secret,
    pub address: Address,
    pub data: Bytes,
}

impl Packet {
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

    pub fn to_bytes(&self) -> io::Result<Bytes> {
        let address_bytes = self.address.to_bytes()?;

        let total_len = SECRET_LENGTH + address_bytes.len() + self.data.len();
        let mut buffer = BytesMut::with_capacity(total_len);

        buffer.put_slice(&self.secret);
        buffer.put_slice(&address_bytes);
        buffer.put_slice(&self.data);

        Ok(buffer.freeze())
    }

    pub fn from_bytes<B: Buf>(buf: &mut B) -> io::Result<Self> {
        if buf.remaining() < SECRET_LENGTH {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Insufficient data for secret",
            ));
        }

        let mut secret = [0u8; SECRET_LENGTH];
        buf.copy_to_slice(&mut secret);

        let address = Address::from_bytes(buf)?;

        let data = buf.copy_to_bytes(buf.remaining());

        Ok(Packet {
            secret,
            address,
            data,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::address::Address;

    use super::*;

    fn create_test_secret() -> Secret {
        let mut secret = [0u8; SECRET_LENGTH];
        for i in 0..SECRET_LENGTH {
            secret[i] = i as u8;
        }
        secret
    }

    #[test]
    fn test_packet_serialization() {
        let secret = create_test_secret();
        let address = Address::Domain("example.com".to_string(), 8080);
        let data = Bytes::from(&b"Hello, World!"[..]);

        let packet = Packet {
            secret,
            address,
            data,
        };

        let bytes = packet.to_bytes().unwrap();
        let mut buf = bytes.as_ref();
        let decoded = Packet::from_bytes(&mut buf).unwrap();

        assert_eq!(decoded.secret, secret);
        match decoded.address {
            Address::Domain(domain, port) => {
                assert_eq!(domain, "example.com");
                assert_eq!(port, 8080);
            }
            _ => panic!("Wrong address type"),
        }

        assert_eq!(decoded.data.to_vec(), b"Hello, World!");
    }

    #[test]
    fn test_packet_from_bytes_insufficient_data() {
        let mut buf = BytesMut::new();
        buf.put_slice(&[0u8; SECRET_LENGTH - 1]);
        assert!(Packet::from_bytes(&mut buf.freeze()).is_err());
    }
}
