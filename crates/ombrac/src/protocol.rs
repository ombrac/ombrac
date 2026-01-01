use std::io;
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};
use std::sync::LazyLock;

use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// Secret key type for authentication (32 bytes, 256 bits).
pub type Secret = [u8; 32];

/// Current protocol version.
pub const PROTOCOL_VERSION: u8 = 0x01;

/// Maximum domain name length in bytes (RFC 1035).
pub const MAX_DOMAIN_LENGTH: usize = 255;

/// Bincode configuration for protocol message serialization.
static BINCODE_CONFIG: LazyLock<bincode::config::Configuration> =
    LazyLock::new(bincode::config::standard);

/// Encodes a protocol message into bytes.
///
/// # Errors
///
/// Returns an error if serialization fails.
pub fn encode<T: Serialize>(message: &T) -> io::Result<Bytes> {
    bincode::serde::encode_to_vec(message, *BINCODE_CONFIG)
        .map(Bytes::from)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("encode error: {e}")))
}

/// Decodes a protocol message from bytes.
///
/// # Errors
///
/// Returns an error if deserialization fails or the data is malformed.
pub fn decode<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> io::Result<T> {
    bincode::serde::borrow_decode_from_slice(bytes, *BINCODE_CONFIG)
        .map(|(msg, _)| msg)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("decode error: {e}")))
}

/// Client authentication message containing credentials and configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientHello {
    /// Protocol version the client supports.
    pub version: u8,
    /// Authentication secret (32-byte hash of the configured secret).
    pub secret: Secret,
    /// Optional protocol extensions and configuration (opaque to the protocol).
    #[serde(with = "serde_bytes")]
    pub options: Bytes,
}

/// Client connection request to establish a tunnel to a destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientConnect {
    /// Destination address to connect to (IP or domain name).
    pub address: Address,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerAuthResponse {
    Ok,
    Err,
}

/// UDP packet representation with support for fragmentation.
///
/// Large UDP packets are automatically fragmented when they exceed the
/// transport layer's maximum datagram size. Fragments are reassembled
/// on the receiving side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UdpPacket {
    /// Complete unfragmented packet.
    Unfragmented {
        /// Unique session identifier for this UDP session.
        session_id: u64,
        /// Destination address for this packet.
        address: Address,
        /// Packet payload.
        #[serde(with = "serde_bytes")]
        data: Bytes,
    },
    /// Fragment of a larger packet.
    Fragmented {
        /// Unique session identifier for this UDP session.
        session_id: u64,
        /// Unique fragment identifier for this fragmentation operation.
        fragment_id: u32,
        /// Zero-based index of this fragment within the packet.
        fragment_index: u16,
        /// Total number of fragments in this packet.
        fragment_count: u16,
        /// Destination address (only present in the first fragment).
        address: Option<Address>,
        /// Fragment payload.
        #[serde(with = "serde_bytes")]
        data: Bytes,
    },
}

impl UdpPacket {
    /// Calculates the overhead for a fragmented packet.
    ///
    /// This includes the fixed overhead (discriminant, session_id, fragment_id,
    /// fragment_index, fragment_count) plus the maximum possible address overhead.
    ///
    /// Returns the maximum overhead in bytes for fragmentation calculations.
    pub fn fragmented_overhead() -> usize {
        // Discriminant (1 byte) + session_id (8 bytes) + fragment_id (4 bytes) +
        // fragment_index (2 bytes) + fragment_count (2 bytes)
        const FIXED_OVERHEAD: usize = 1 + 8 + 4 + 2 + 2;
        // Maximum address overhead: discriminant (1 byte) + length (2 bytes) +
        // max domain (255 bytes) + port (2 bytes)
        const MAX_ADDRESS_OVERHEAD: usize = 1 + 2 + MAX_DOMAIN_LENGTH + 2;
        FIXED_OVERHEAD + MAX_ADDRESS_OVERHEAD
    }

    /// Splits a large packet into fragments.
    ///
    /// # Arguments
    ///
    /// * `session_id` - The session identifier for this packet
    /// * `address` - The destination address (included only in the first fragment)
    /// * `data` - The packet data to fragment
    /// * `max_payload_size` - Maximum payload size per fragment (after overhead)
    /// * `fragment_id` - Unique identifier for this fragmentation operation
    ///
    /// # Returns
    ///
    /// An iterator over `UdpPacket::Fragmented` packets.
    pub fn split_packet(
        session_id: u64,
        address: Address,
        data: Bytes,
        max_payload_size: usize,
        fragment_id: u32,
    ) -> impl Iterator<Item = UdpPacket> {
        // Split data into chunks, ensuring each chunk fits within max_payload_size
        let data_chunks: Vec<Bytes> = data
            .chunks(max_payload_size)
            .map(Bytes::copy_from_slice)
            .collect();
        let fragment_count = data_chunks.len() as u16;

        // Ensure fragment_count fits in u16
        assert!(fragment_count > 0, "fragment_count must be greater than 0");

        data_chunks.into_iter().enumerate().map(move |(i, chunk)| {
            let fragment_index = i as u16;
            UdpPacket::Fragmented {
                session_id,
                fragment_id,
                fragment_index,
                fragment_count,
                // Only include address in the first fragment to save bandwidth
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

/// Response to a client's connection request.
///
/// This message is sent by the server after attempting to connect to the
/// destination address. It indicates whether the connection was successful
/// or failed, allowing the client to properly handle TCP state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerConnectResponse {
    /// Connection to the destination was successful.
    Ok,
    /// Connection to the destination failed.
    ///
    /// The error message provides details about why the connection failed,
    /// which helps the client understand the failure context and avoid
    /// retry storms in application-layer protocols.
    Err {
        /// Error kind that categorizes the failure
        kind: ConnectErrorKind,
        /// Human-readable error message
        message: String,
    },
}

/// Categorizes connection errors to help clients handle them appropriately.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectErrorKind {
    /// Connection refused by the destination
    ConnectionRefused,
    /// Network unreachable
    NetworkUnreachable,
    /// Host unreachable
    HostUnreachable,
    /// Connection timed out
    TimedOut,
    #[serde(other)]
    Other,
}

impl ConnectErrorKind {
    /// Converts an `io::Error` to a `ConnectErrorKind` based on the error kind.
    ///
    /// This function maps standard IO error kinds to protocol error kinds,
    /// ensuring consistent error handling across the codebase. DNS resolution
    /// failures are categorized as `Other` since they can manifest with
    /// different error kinds depending on the platform.
    pub fn from_io_error(error: &io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::ConnectionRefused => ConnectErrorKind::ConnectionRefused,
            io::ErrorKind::NetworkUnreachable => ConnectErrorKind::NetworkUnreachable,
            io::ErrorKind::HostUnreachable => ConnectErrorKind::HostUnreachable,
            io::ErrorKind::TimedOut => ConnectErrorKind::TimedOut,
            // All other errors, including DNS resolution failures (NotFound, etc.),
            // are categorized as Other
            _ => ConnectErrorKind::Other,
        }
    }
}

/// Network address representation supporting IPv4, IPv6, and domain names.
///
/// This type is used throughout the protocol to specify destination addresses
/// for both TCP and UDP connections.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub enum Address {
    /// IPv4 socket address.
    SocketV4(SocketAddrV4),
    /// IPv6 socket address.
    SocketV6(SocketAddrV6),
    /// Domain name with port (domain name is limited to 255 bytes per RFC 1035).
    Domain(#[serde(with = "serde_bytes")] Bytes, u16),
}

impl Address {
    /// Resolves this address to a `SocketAddr`.
    ///
    /// For IP addresses, this is a no-op. For domain names, this performs
    /// asynchronous DNS resolution.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The domain name contains invalid UTF-8
    /// - DNS resolution fails
    /// - No addresses are found for the domain
    pub async fn to_socket_addr(&self) -> io::Result<SocketAddr> {
        match self {
            Self::SocketV4(addr) => Ok((*addr).into()),
            Self::SocketV6(addr) => Ok((*addr).into()),
            Self::Domain(domain, port) => {
                let domain_str = std::str::from_utf8(domain).map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "domain name contains invalid utf-8 characters",
                    )
                })?;

                tokio::net::lookup_host((domain_str, *port))
                    .await?
                    .next()
                    .ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::NotFound,
                            format!("domain name '{}' could not be resolved", domain_str),
                        )
                    })
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
                    "domain name cannot be empty",
                ));
            }

            if domain.len() > MAX_DOMAIN_LENGTH {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "domain name is too long: {} bytes (max {})",
                        domain.len(),
                        MAX_DOMAIN_LENGTH
                    ),
                ));
            }

            return Ok(Address::Domain(
                Bytes::copy_from_slice(domain.as_bytes()),
                port,
            ));
        }

        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid address format: {}", value),
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
