use std::io;
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};
use std::sync::LazyLock;

use bytes::Bytes;
use serde::{Deserialize, Serialize};

pub type Secret = [u8; 32];

pub const PROTOCOLS_VERSION: u8 = 0x01;

static BINCODE_CONFIG: LazyLock<bincode::config::Configuration> =
    LazyLock::new(bincode::config::standard);

pub fn encode<T: Serialize>(message: &T) -> io::Result<Bytes> {
    bincode::serde::encode_to_vec(message, *BINCODE_CONFIG)
        .map(Bytes::from)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
}

pub fn decode<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> io::Result<T> {
    bincode::serde::borrow_decode_from_slice(bytes, *BINCODE_CONFIG)
        .map(|(msg, _)| msg)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientHello {
    pub version: u8,
    pub secret: Secret,
    #[serde(with = "serde_bytes")]
    pub options: Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientConnect {
    pub address: Address,
}


#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerHandshakeResponse {
    Ok,
    Err(HandshakeError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UdpPacket {
    Unfragmented {
        session_id: u64,
        address: Address,
        #[serde(with = "serde_bytes")]
        data: Bytes,
    },
    Fragmented {
        session_id: u64,
        fragment_id: u32,
        fragment_index: u16,
        fragment_count: u16,
        address: Option<Address>,
        #[serde(with = "serde_bytes")]
        data: Bytes,
    },
}

impl UdpPacket {
    pub fn fragmented_overhead() -> usize {
        // Type + u64 + u32 + u16 + u16
        let fixed_overhead = 1 + 8 + 4 + 2 + 2;
        // 1 byte tag + 2 bytes len + 255 bytes domain + 2 bytes port
        const MAX_ADDRESS_OVERHEAD: usize = 260;
        fixed_overhead + MAX_ADDRESS_OVERHEAD
    }

    pub fn split_packet(
        session_id: u64,
        address: Address,
        data: Bytes,
        max_payload_size: usize,
        fragment_id: u32,
    ) -> impl Iterator<Item = UdpPacket> {
        let data_chunks: Vec<Bytes> = data
            .chunks(max_payload_size)
            .map(Bytes::copy_from_slice)
            .collect();
        let fragment_count = data_chunks.len() as u16;

        data_chunks.into_iter().enumerate().map(move |(i, chunk)| {
            let fragment_index = i as u16;
            UdpPacket::Fragmented {
                session_id,
                fragment_id,
                fragment_index,
                fragment_count,
                address: if fragment_index == 0 {
                    Some(address.clone())
                } else {
                    None
                },
                data: chunk,
            }
        })
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum HandshakeError {
    UnsupportedVersion,
    InvalidSecret,
    InternalServerError,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub enum Address {
    SocketV4(SocketAddrV4),
    SocketV6(SocketAddrV6),
    Domain(#[serde(with = "serde_bytes")] Bytes, u16),
}

impl Address {
    pub async fn to_socket_addr(&self) -> io::Result<SocketAddr> {
        match self {
            Self::SocketV4(addr) => Ok((*addr).into()),
            Self::SocketV6(addr) => Ok((*addr).into()),
            Self::Domain(domain, port) => {
                let domain_str = std::str::from_utf8(domain).map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Domain name contains invalid UTF-8 characters",
                    )
                })?;

                match tokio::net::lookup_host((domain_str, *port)).await?.next() {
                    Some(addr) => Ok(addr),
                    None => Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("Domain name '{}' could not be resolved", domain_str),
                    )),
                }
            }
        }
    }
}

impl From<SocketAddr> for Address {
    fn from(value: SocketAddr) -> Self {
        match value {
            SocketAddr::V4(addr) => Self::SocketV4(addr),
            SocketAddr::V6(addr) => Self::SocketV6(addr),
        }
    }
}

impl TryFrom<&str> for Address {
    type Error = io::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if let Ok(addr) = value.parse::<SocketAddr>() {
            return Ok(Address::from(addr));
        }

        if let Some((domain, port_str)) = value.rsplit_once(':')
            && let Ok(port) = port_str.parse::<u16>()
        {
            if domain.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Domain name cannot be empty",
                ));
            }

            if domain.len() > 255 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Domain name is too long: {} bytes (max 255)", domain.len()),
                ));
            }

            return Ok(Address::Domain(
                Bytes::copy_from_slice(domain.as_bytes()),
                port,
            ));
        }

        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Invalid address format: {}", value),
        ))
    }
}

impl TryFrom<String> for Address {
    type Error = io::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Address::try_from(value.as_str())
    }
}

impl From<(String, u16)> for Address {
    fn from(value: (String, u16)) -> Self {
        Address::Domain(Bytes::from(value.0), value.1)
    }
}

impl From<(&str, u16)> for Address {
    fn from(value: (&str, u16)) -> Self {
        Address::Domain(Bytes::copy_from_slice(value.0.as_bytes()), value.1)
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Domain(domain, port) => {
                write!(f, "{}:{}", String::from_utf8_lossy(domain), port)
            }
            Self::SocketV4(addr) => write!(f, "{}", addr),
            Self::SocketV6(addr) => write!(f, "{}", addr),
        }
    }
}

mod serde_bytes {
    use bytes::Bytes;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &Bytes, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(bytes)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Bytes, D::Error>
    where
        D: Deserializer<'de>,
    {
        let vec: Vec<u8> = Vec::deserialize(deserializer)?;
        Ok(Bytes::from(vec))
    }
}

#[macro_export]
macro_rules! impl_message_serde {
    ($struct_name:ident) => {
        impl $struct_name {
            pub fn encode(&self) -> io::Result<Bytes> {
                encode(self)
            }

            pub fn decode(bytes: &[u8]) -> io::Result<Self> {
                decode(bytes)
            }
        }
    };
}

impl_message_serde!(ClientHello);
impl_message_serde!(UdpPacket);
impl_message_serde!(Address);
