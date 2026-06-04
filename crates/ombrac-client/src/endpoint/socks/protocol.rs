//! Minimal, allocation-conscious SOCKS5 wire protocol implementation.
//!
//! Only the subset required by the ombrac client endpoint is implemented:
//! the method-selection handshake, the `CONNECT`/`UDP ASSOCIATE` requests,
//! server replies, and the UDP request header. `BIND` is parsed but rejected
//! by the handler.
//!
//! References: [RFC 1928](https://datatracker.ietf.org/doc/html/rfc1928).

use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

use bytes::{Buf, BufMut, Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt};

use ombrac::protocol::Address as OmbracAddress;

/// SOCKS protocol version 5.
pub const VERSION: u8 = 0x05;

// Address type tags (ATYP).
const ATYP_IPV4: u8 = 0x01;
const ATYP_DOMAIN: u8 = 0x03;
const ATYP_IPV6: u8 = 0x04;

// Request commands (CMD).
const CMD_CONNECT: u8 = 0x01;
const CMD_BIND: u8 = 0x02;
const CMD_ASSOCIATE: u8 = 0x03;

#[inline]
fn invalid_data(msg: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

/// Authentication method codes (the single `METHOD` octet of RFC 1928 §3).
pub mod method {
    /// No authentication required.
    pub const NO_AUTHENTICATION: u8 = 0x00;
    /// No acceptable method (sent by the server to reject the client).
    pub const NO_ACCEPTABLE_METHOD: u8 = 0xFF;
}

/// A SOCKS5 address (`ATYP` + `DST.ADDR` + `DST.PORT`).
///
/// Domains are kept as raw bytes to avoid an intermediate `String` allocation
/// on the hot path; they are only validated as UTF-8 when converted to an
/// [`OmbracAddress`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Address {
    IPv4(SocketAddrV4),
    IPv6(SocketAddrV6),
    Domain(Bytes, u16),
}

impl Address {
    /// Unspecified IPv4 address (`0.0.0.0:0`), used for replies whose bound
    /// address is irrelevant.
    pub const UNSPECIFIED: Address =
        Address::IPv4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0));

    /// Reads an address from an async stream.
    pub async fn read<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Self> {
        let atyp = reader.read_u8().await?;
        match atyp {
            ATYP_IPV4 => {
                let mut buf = [0u8; 6];
                reader.read_exact(&mut buf).await?;
                let ip = Ipv4Addr::new(buf[0], buf[1], buf[2], buf[3]);
                let port = u16::from_be_bytes([buf[4], buf[5]]);
                Ok(Address::IPv4(SocketAddrV4::new(ip, port)))
            }
            ATYP_IPV6 => {
                let mut buf = [0u8; 18];
                reader.read_exact(&mut buf).await?;
                let mut octets = [0u8; 16];
                octets.copy_from_slice(&buf[..16]);
                let port = u16::from_be_bytes([buf[16], buf[17]]);
                Ok(Address::IPv6(SocketAddrV6::new(
                    Ipv6Addr::from(octets),
                    port,
                    0,
                    0,
                )))
            }
            ATYP_DOMAIN => {
                let len = reader.read_u8().await? as usize;
                if len == 0 {
                    return Err(invalid_data("empty domain"));
                }
                let mut buf = vec![0u8; len + 2];
                reader.read_exact(&mut buf).await?;
                let port = u16::from_be_bytes([buf[len], buf[len + 1]]);
                buf.truncate(len);
                Ok(Address::Domain(Bytes::from(buf), port))
            }
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid address type: {other}"),
            )),
        }
    }

    /// Parses an address from an in-memory buffer (used for UDP datagrams).
    pub fn from_buf<B: Buf>(buf: &mut B) -> io::Result<Self> {
        if buf.remaining() < 1 {
            return Err(invalid_data("insufficient data for address type"));
        }
        let atyp = buf.get_u8();
        match atyp {
            ATYP_IPV4 => {
                if buf.remaining() < 6 {
                    return Err(invalid_data("insufficient data for IPv4 address"));
                }
                let mut ip = [0u8; 4];
                buf.copy_to_slice(&mut ip);
                let port = buf.get_u16();
                Ok(Address::IPv4(SocketAddrV4::new(Ipv4Addr::from(ip), port)))
            }
            ATYP_IPV6 => {
                if buf.remaining() < 18 {
                    return Err(invalid_data("insufficient data for IPv6 address"));
                }
                let mut ip = [0u8; 16];
                buf.copy_to_slice(&mut ip);
                let port = buf.get_u16();
                Ok(Address::IPv6(SocketAddrV6::new(
                    Ipv6Addr::from(ip),
                    port,
                    0,
                    0,
                )))
            }
            ATYP_DOMAIN => {
                if buf.remaining() < 1 {
                    return Err(invalid_data("insufficient data for domain length"));
                }
                let len = buf.get_u8() as usize;
                if len == 0 {
                    return Err(invalid_data("empty domain"));
                }
                if buf.remaining() < len + 2 {
                    return Err(invalid_data("insufficient data for domain name"));
                }
                let domain = buf.copy_to_bytes(len);
                let port = buf.get_u16();
                Ok(Address::Domain(domain, port))
            }
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid address type: {other}"),
            )),
        }
    }

    /// Encodes the address into `dst`.
    pub fn encode(&self, dst: &mut BytesMut) {
        match self {
            Address::IPv4(addr) => {
                dst.put_u8(ATYP_IPV4);
                dst.put_slice(&addr.ip().octets());
                dst.put_u16(addr.port());
            }
            Address::IPv6(addr) => {
                dst.put_u8(ATYP_IPV6);
                dst.put_slice(&addr.ip().octets());
                dst.put_u16(addr.port());
            }
            Address::Domain(domain, port) => {
                dst.put_u8(ATYP_DOMAIN);
                dst.put_u8(domain.len() as u8);
                dst.put_slice(domain);
                dst.put_u16(*port);
            }
        }
    }

    /// Serialized length in bytes.
    #[inline]
    pub fn encoded_len(&self) -> usize {
        match self {
            Address::IPv4(_) => 1 + 4 + 2,
            Address::IPv6(_) => 1 + 16 + 2,
            Address::Domain(domain, _) => 1 + 1 + domain.len() + 2,
        }
    }
}

impl From<SocketAddr> for Address {
    #[inline]
    fn from(value: SocketAddr) -> Self {
        match value {
            SocketAddr::V4(addr) => Address::IPv4(addr),
            SocketAddr::V6(addr) => Address::IPv6(addr),
        }
    }
}

impl From<Address> for OmbracAddress {
    #[inline]
    fn from(value: Address) -> Self {
        match value {
            Address::IPv4(addr) => OmbracAddress::SocketV4(addr),
            Address::IPv6(addr) => OmbracAddress::SocketV6(addr),
            Address::Domain(domain, port) => OmbracAddress::Domain(domain, port),
        }
    }
}

impl TryFrom<OmbracAddress> for Address {
    type Error = io::Error;

    #[inline]
    fn try_from(value: OmbracAddress) -> Result<Self, Self::Error> {
        Ok(match value {
            OmbracAddress::SocketV4(addr) => Address::IPv4(addr),
            OmbracAddress::SocketV6(addr) => Address::IPv6(addr),
            OmbracAddress::Domain(domain, port) => {
                if domain.len() > 255 {
                    return Err(invalid_data("domain exceeds maximum length"));
                }
                Address::Domain(domain, port)
            }
        })
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Address::IPv4(addr) => write!(f, "{addr}"),
            Address::IPv6(addr) => write!(f, "{addr}"),
            Address::Domain(domain, port) => {
                write!(f, "{}:{}", String::from_utf8_lossy(domain), port)
            }
        }
    }
}

/// A SOCKS5 client request (after the leading `VER` byte).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Connect(Address),
    Bind(Address),
    Associate(Address),
}

impl Request {
    /// Reads `VER CMD RSV ATYP ADDR PORT` from the stream.
    pub async fn read<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Self> {
        let mut header = [0u8; 3];
        reader.read_exact(&mut header).await?;

        if header[0] != VERSION {
            return Err(invalid_data("unsupported SOCKS version"));
        }

        let address = Address::read(reader).await?;
        match header[1] {
            CMD_CONNECT => Ok(Request::Connect(address)),
            CMD_BIND => Ok(Request::Bind(address)),
            CMD_ASSOCIATE => Ok(Request::Associate(address)),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid request command: {other}"),
            )),
        }
    }
}

/// A SOCKS5 server reply code.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Reply {
    Succeeded,
    GeneralFailure,
    ConnectionNotAllowed,
    NetworkUnreachable,
    HostUnreachable,
    ConnectionRefused,
    TtlExpired,
    CommandNotSupported,
}

impl Reply {
    #[inline]
    fn as_u8(&self) -> u8 {
        match self {
            Reply::Succeeded => 0x00,
            Reply::GeneralFailure => 0x01,
            Reply::ConnectionNotAllowed => 0x02,
            Reply::NetworkUnreachable => 0x03,
            Reply::HostUnreachable => 0x04,
            Reply::ConnectionRefused => 0x05,
            Reply::TtlExpired => 0x06,
            Reply::CommandNotSupported => 0x07,
        }
    }

    /// Maps an I/O error encountered while connecting to the closest SOCKS5
    /// reply code, following RFC 1928 semantics.
    pub fn from_connect_error(err: &io::Error) -> Self {
        match err.kind() {
            io::ErrorKind::ConnectionRefused => Reply::ConnectionRefused,
            io::ErrorKind::TimedOut => Reply::TtlExpired,
            io::ErrorKind::HostUnreachable => Reply::HostUnreachable,
            io::ErrorKind::NetworkUnreachable => Reply::NetworkUnreachable,
            io::ErrorKind::PermissionDenied => Reply::ConnectionNotAllowed,
            _ => Reply::GeneralFailure,
        }
    }
}

/// Encodes a server reply `VER REP RSV ATYP BND.ADDR BND.PORT`.
pub fn encode_reply(reply: Reply, address: &Address) -> BytesMut {
    let mut bytes = BytesMut::with_capacity(3 + address.encoded_len());
    bytes.put_u8(VERSION);
    bytes.put_u8(reply.as_u8());
    bytes.put_u8(0x00);
    address.encode(&mut bytes);
    bytes
}

/// A SOCKS5 UDP request/response header plus payload.
///
/// ```text
///  +-----+------+------+----------+----------+----------+
///  | RSV | FRAG | ATYP | DST.ADDR | DST.PORT |   DATA   |
///  +-----+------+------+----------+----------+----------+
///  |  2  |  1   |  1   | Variable |    2     | Variable |
///  +-----+------+------+----------+----------+----------+
/// ```
#[cfg(feature = "datagram")]
#[derive(Debug)]
pub struct UdpPacket {
    pub frag: u8,
    pub address: Address,
    pub data: Bytes,
}

#[cfg(feature = "datagram")]
impl UdpPacket {
    /// Parses a UDP datagram payload.
    pub fn from_buf(mut buf: Bytes) -> io::Result<Self> {
        if buf.remaining() < 3 {
            return Err(invalid_data("insufficient data for UDP header"));
        }
        buf.advance(2); // RSV
        let frag = buf.get_u8();
        let address = Address::from_buf(&mut buf)?;
        Ok(Self {
            frag,
            address,
            data: buf,
        })
    }

    /// Builds an unfragmented UDP datagram for `address`/`data`.
    pub fn encode_unfragmented(address: &Address, data: &[u8]) -> Bytes {
        let mut bytes = BytesMut::with_capacity(3 + address.encoded_len() + data.len());
        bytes.put_u8(0x00);
        bytes.put_u8(0x00);
        bytes.put_u8(0x00);
        address.encode(&mut bytes);
        bytes.put_slice(data);
        bytes.freeze()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn method_constants() {
        assert_eq!(method::NO_AUTHENTICATION, 0x00);
        assert_eq!(method::NO_ACCEPTABLE_METHOD, 0xFF);
    }

    fn encode(addr: &Address) -> BytesMut {
        let mut b = BytesMut::new();
        addr.encode(&mut b);
        b
    }

    #[tokio::test]
    async fn address_ipv4_round_trip() {
        let addr = Address::IPv4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 8080));
        let bytes = encode(&addr);
        assert_eq!(bytes.len(), addr.encoded_len());
        assert_eq!(bytes[0], ATYP_IPV4);

        let mut cursor = Cursor::new(bytes.clone());
        assert_eq!(Address::read(&mut cursor).await.unwrap(), addr);

        let mut b = bytes.freeze();
        assert_eq!(Address::from_buf(&mut b).unwrap(), addr);
    }

    #[tokio::test]
    async fn address_ipv6_round_trip() {
        let addr = Address::IPv6(SocketAddrV6::new(
            Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1),
            443,
            0,
            0,
        ));
        let bytes = encode(&addr);
        assert_eq!(bytes.len(), addr.encoded_len());
        assert_eq!(bytes[0], ATYP_IPV6);

        let mut cursor = Cursor::new(bytes.clone());
        assert_eq!(Address::read(&mut cursor).await.unwrap(), addr);

        let mut b = bytes.freeze();
        assert_eq!(Address::from_buf(&mut b).unwrap(), addr);
    }

    #[tokio::test]
    async fn address_domain_round_trip() {
        let addr = Address::Domain(Bytes::from_static(b"example.com"), 8080);
        let bytes = encode(&addr);
        assert_eq!(bytes.len(), addr.encoded_len());
        assert_eq!(bytes[0], ATYP_DOMAIN);
        assert_eq!(bytes[1], 11);

        let mut cursor = Cursor::new(bytes.clone());
        assert_eq!(Address::read(&mut cursor).await.unwrap(), addr);

        let mut b = bytes.freeze();
        assert_eq!(Address::from_buf(&mut b).unwrap(), addr);
    }

    #[tokio::test]
    async fn address_read_rejects_invalid_atyp() {
        let mut cursor = Cursor::new(vec![0x02u8]);
        assert!(Address::read(&mut cursor).await.is_err());
    }

    #[tokio::test]
    async fn address_read_rejects_empty_domain() {
        let mut cursor = Cursor::new(vec![ATYP_DOMAIN, 0x00]);
        assert!(Address::read(&mut cursor).await.is_err());
    }

    #[test]
    fn address_from_buf_rejects_truncated_ipv4() {
        let mut b = Bytes::from_static(&[ATYP_IPV4, 192, 168]);
        assert!(Address::from_buf(&mut b).is_err());
    }

    #[test]
    fn address_from_buf_rejects_truncated_domain() {
        // length says 5 but only 3 bytes follow
        let mut b = Bytes::from_static(&[ATYP_DOMAIN, 0x05, b'a', b'b', b'c']);
        assert!(Address::from_buf(&mut b).is_err());
    }

    #[test]
    fn address_ombrac_conversion_round_trip() {
        let cases = [
            Address::IPv4(SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 1), 80)),
            Address::IPv6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, 443, 0, 0)),
            Address::Domain(Bytes::from_static(b"example.org"), 1234),
        ];
        for addr in cases {
            let ombrac: OmbracAddress = addr.clone().into();
            let back = Address::try_from(ombrac).unwrap();
            assert_eq!(addr, back);
        }
    }

    #[test]
    fn address_display() {
        assert_eq!(
            Address::IPv4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 8080)).to_string(),
            "127.0.0.1:8080"
        );
        assert_eq!(
            Address::Domain(Bytes::from_static(b"example.com"), 80).to_string(),
            "example.com:80"
        );
    }

    #[tokio::test]
    async fn request_connect_ipv4() {
        let mut buf = vec![VERSION, CMD_CONNECT, 0x00, ATYP_IPV4, 192, 168, 1, 1];
        buf.extend_from_slice(&80u16.to_be_bytes());
        let mut cursor = Cursor::new(buf);
        let request = Request::read(&mut cursor).await.unwrap();
        assert_eq!(
            request,
            Request::Connect(Address::IPv4(SocketAddrV4::new(
                Ipv4Addr::new(192, 168, 1, 1),
                80
            )))
        );
    }

    #[tokio::test]
    async fn request_associate_domain() {
        let mut buf = vec![VERSION, CMD_ASSOCIATE, 0x00, ATYP_DOMAIN, 11];
        buf.extend_from_slice(b"example.com");
        buf.extend_from_slice(&8080u16.to_be_bytes());
        let mut cursor = Cursor::new(buf);
        let request = Request::read(&mut cursor).await.unwrap();
        assert_eq!(
            request,
            Request::Associate(Address::Domain(Bytes::from_static(b"example.com"), 8080))
        );
    }

    #[tokio::test]
    async fn request_rejects_bad_version() {
        let buf = vec![0x04, CMD_CONNECT, 0x00, ATYP_IPV4, 1, 2, 3, 4, 0, 80];
        let mut cursor = Cursor::new(buf);
        assert!(Request::read(&mut cursor).await.is_err());
    }

    #[tokio::test]
    async fn request_rejects_bad_command() {
        let buf = vec![VERSION, 0xEE, 0x00, ATYP_IPV4, 1, 2, 3, 4, 0, 80];
        let mut cursor = Cursor::new(buf);
        assert!(Request::read(&mut cursor).await.is_err());
    }

    #[test]
    fn reply_encoding() {
        let addr = Address::IPv4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 1080));
        let bytes = encode_reply(Reply::Succeeded, &addr);
        assert_eq!(bytes[0], VERSION);
        assert_eq!(bytes[1], 0x00);
        assert_eq!(bytes[2], 0x00);
        assert_eq!(bytes[3], ATYP_IPV4);
        assert_eq!(&bytes[4..8], &[127, 0, 0, 1]);
        assert_eq!(&bytes[8..10], &1080u16.to_be_bytes());
    }

    #[test]
    fn reply_from_connect_error() {
        assert_eq!(
            Reply::from_connect_error(&io::Error::from(io::ErrorKind::ConnectionRefused)),
            Reply::ConnectionRefused
        );
        assert_eq!(
            Reply::from_connect_error(&io::Error::from(io::ErrorKind::TimedOut)),
            Reply::TtlExpired
        );
        assert_eq!(
            Reply::from_connect_error(&io::Error::other("boom")),
            Reply::GeneralFailure
        );
    }

    #[cfg(feature = "datagram")]
    #[test]
    fn udp_packet_round_trip() {
        let address = Address::IPv4(SocketAddrV4::new(Ipv4Addr::new(8, 8, 8, 8), 53));
        let payload = b"hello world";
        let encoded = UdpPacket::encode_unfragmented(&address, payload);

        // RSV(2) + FRAG(1) + addr + data
        assert_eq!(encoded[0], 0x00);
        assert_eq!(encoded[1], 0x00);
        assert_eq!(encoded[2], 0x00);

        let parsed = UdpPacket::from_buf(encoded).unwrap();
        assert_eq!(parsed.frag, 0);
        assert_eq!(parsed.address, address);
        assert_eq!(&parsed.data[..], payload);
    }

    #[cfg(feature = "datagram")]
    #[test]
    fn udp_packet_rejects_short_header() {
        assert!(UdpPacket::from_buf(Bytes::from_static(&[0x00, 0x00])).is_err());
    }

    #[cfg(feature = "datagram")]
    #[test]
    fn udp_packet_preserves_frag() {
        let address = Address::IPv4(SocketAddrV4::new(Ipv4Addr::new(1, 1, 1, 1), 53));
        let mut raw = BytesMut::new();
        raw.put_u8(0x00);
        raw.put_u8(0x00);
        raw.put_u8(0x07); // frag
        address.encode(&mut raw);
        raw.put_slice(b"data");
        let parsed = UdpPacket::from_buf(raw.freeze()).unwrap();
        assert_eq!(parsed.frag, 0x07);
        assert_eq!(&parsed.data[..], b"data");
    }
}
