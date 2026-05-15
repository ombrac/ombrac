use std::net::SocketAddr;
use std::sync::Arc;

use etherparse::PacketBuilder;
use tokio::sync::mpsc;

use crate::buffer::BufferPool;
use crate::stack::IpPacket;
use crate::stack::{NetStackConfig, Packet};
use crate::{error, trace};

pub struct UdpPacket {
    pub data: Packet,
    pub src_addr: SocketAddr,
    pub dst_addr: SocketAddr,
}

impl<T> From<(T, SocketAddr, SocketAddr)> for UdpPacket
where
    T: Into<Packet>,
{
    fn from((data, src_addr, dst_addr): (T, SocketAddr, SocketAddr)) -> Self {
        UdpPacket {
            data: data.into(),
            src_addr,
            dst_addr,
        }
    }
}

impl UdpPacket {
    pub fn data(&self) -> &[u8] {
        self.data.data()
    }
}

pub struct UdpTunnel {
    inbound: mpsc::Receiver<Packet>,
    outbound: mpsc::Sender<Packet>,
    buffer_pool: Arc<BufferPool>,
    config: Arc<NetStackConfig>,
}

impl UdpTunnel {
    pub fn new(
        config: Arc<NetStackConfig>,
        inbound: mpsc::Receiver<Packet>,
        outbound: mpsc::Sender<Packet>,
        buffer_pool: Arc<BufferPool>,
    ) -> Self {
        Self {
            inbound,
            outbound,
            buffer_pool,
            config,
        }
    }

    pub fn split(self) -> (SplitRead, SplitWrite) {
        let read = SplitRead { recv: self.inbound };
        let write = SplitWrite {
            config: self.config,
            send: self.outbound,
            buffer_pool: self.buffer_pool,
        };
        (read, write)
    }
}

pub struct SplitRead {
    recv: mpsc::Receiver<Packet>,
}

impl SplitRead {
    pub async fn recv(&mut self) -> Option<UdpPacket> {
        self.recv.recv().await.and_then(|data| {
            let original_bytes = data.into_bytes();

            // Compute the IP header length so we can slice the payload from the
            // original Bytes without relying on pointer arithmetic against
            // smoltcp's slice references.
            let ip_header_len = match IpPacket::new_checked(original_bytes.as_ref()) {
                Ok(IpPacket::Ipv4(p)) => p.header_len() as usize,
                Ok(IpPacket::Ipv6(_)) => smoltcp::wire::IPV6_HEADER_LEN,
                Err(_e) => {
                    error!("invalid IP packet: {_e}");
                    return None;
                }
            };

            let (src_ip, dst_ip) = {
                let packet = IpPacket::new_checked(original_bytes.as_ref()).ok()?;
                (packet.src_addr(), packet.dst_addr())
            };

            // Re-parse the UDP header from the IP payload, but track its byte
            // range inside `original_bytes` so we can slice it cheaply.
            const UDP_HEADER_LEN: usize = 8;
            if original_bytes.len() < ip_header_len + UDP_HEADER_LEN {
                error!("packet too short for UDP header");
                return None;
            }

            let udp_segment = &original_bytes[ip_header_len..];
            let udp_packet = match smoltcp::wire::UdpPacket::new_checked(udp_segment) {
                Ok(p) => p,
                Err(_e) => {
                    error!(
                        "invalid UDP packet: {_e}, src_ip: {src_ip}, dst_ip: {dst_ip}"
                    );
                    return None;
                }
            };
            let src_port = udp_packet.src_port();
            let dst_port = udp_packet.dst_port();
            let payload_len = udp_packet.payload().len();

            let payload_start = ip_header_len + UDP_HEADER_LEN;
            let payload_end = payload_start + payload_len;
            if payload_end > original_bytes.len() {
                error!("UDP payload length exceeds packet bounds");
                return None;
            }
            let payload_bytes = original_bytes.slice(payload_start..payload_end);

            let src_addr = SocketAddr::new(src_ip, src_port);
            let dst_addr = SocketAddr::new(dst_ip, dst_port);

            trace!("created UDP socket for {src_addr} <-> {dst_addr}");

            Some(UdpPacket {
                data: Packet::new(payload_bytes),
                src_addr,
                dst_addr,
            })
        })
    }
}

#[derive(Clone)]
pub struct SplitWrite {
    config: Arc<NetStackConfig>,
    send: mpsc::Sender<Packet>,
    buffer_pool: Arc<BufferPool>,
}

impl SplitWrite {
    pub async fn send(&mut self, packet: UdpPacket) -> Result<(), std::io::Error> {
        let ttl = self.config.ip_ttl;
        let builder = match (packet.src_addr, packet.dst_addr) {
            (SocketAddr::V4(src), SocketAddr::V4(dst)) => {
                PacketBuilder::ipv4(src.ip().octets(), dst.ip().octets(), ttl)
                    .udp(src.port(), dst.port())
            }
            (SocketAddr::V6(src), SocketAddr::V6(dst)) => {
                PacketBuilder::ipv6(src.ip().octets(), dst.ip().octets(), ttl)
                    .udp(src.port(), dst.port())
            }
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "UDP socket only supports IPv4 and IPv6",
                ));
            }
        };

        let mut buffer = self.buffer_pool.get(builder.size(packet.data.data().len()));
        builder
            .write(&mut buffer, packet.data.data())
            .map_err(std::io::Error::other)?;
        let final_bytes = buffer.split().freeze();

        match self.send.send(Packet::new(final_bytes)).await {
            Ok(()) => Ok(()),
            Err(err) => Err(std::io::Error::other(format!("send error: {err}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::BufferPool;
    use bytes::Bytes;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6};
    use std::sync::Arc;

    fn make_config() -> Arc<NetStackConfig> {
        Arc::new(NetStackConfig::default())
    }

    fn make_pool() -> Arc<BufferPool> {
        Arc::new(BufferPool::new(8, 1500))
    }

    #[test]
    fn udp_packet_from_tuple_constructs_correctly() {
        let data = Bytes::from_static(b"payload");
        let src = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 1000);
        let dst = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)), 53);
        let pkt: UdpPacket = (data.clone(), src, dst).into();
        assert_eq!(pkt.data(), data.as_ref());
        assert_eq!(pkt.src_addr, src);
        assert_eq!(pkt.dst_addr, dst);
    }

    #[tokio::test]
    async fn split_write_emits_ipv4_udp_frame() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Packet>(4);
        let mut writer = SplitWrite {
            config: make_config(),
            send: tx,
            buffer_pool: make_pool(),
        };

        let payload = Bytes::from_static(b"hello dns");
        let src = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 1), 5000));
        let dst = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(8, 8, 8, 8), 53));
        let pkt = UdpPacket {
            data: Packet::new(payload.clone()),
            src_addr: src,
            dst_addr: dst,
        };

        writer.send(pkt).await.unwrap();
        let out = rx.recv().await.unwrap();

        // Validate that the produced bytes form a proper IPv4+UDP packet.
        let ip = crate::stack::IpPacket::new_checked(out.data()).unwrap();
        assert_eq!(ip.protocol(), smoltcp::wire::IpProtocol::Udp);
        assert_eq!(ip.src_addr().to_string(), "10.0.0.1");
        assert_eq!(ip.dst_addr().to_string(), "8.8.8.8");
    }

    #[tokio::test]
    async fn split_write_emits_ipv6_udp_frame() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Packet>(4);
        let mut writer = SplitWrite {
            config: make_config(),
            send: tx,
            buffer_pool: make_pool(),
        };

        let payload = Bytes::from_static(b"ping");
        let src = SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, 1000, 0, 0));
        let dst = SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, 53, 0, 0));
        let pkt = UdpPacket {
            data: Packet::new(payload),
            src_addr: src,
            dst_addr: dst,
        };

        writer.send(pkt).await.unwrap();
        let out = rx.recv().await.unwrap();
        let ip = crate::stack::IpPacket::new_checked(out.data()).unwrap();
        assert_eq!(ip.protocol(), smoltcp::wire::IpProtocol::Udp);
    }

    #[tokio::test]
    async fn split_write_rejects_mixed_address_families() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<Packet>(4);
        let mut writer = SplitWrite {
            config: make_config(),
            send: tx,
            buffer_pool: make_pool(),
        };

        let src = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 1), 1000));
        let dst = SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, 53, 0, 0));
        let pkt = UdpPacket {
            data: Packet::new(Bytes::from_static(b"x")),
            src_addr: src,
            dst_addr: dst,
        };

        let err = writer.send(pkt).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[tokio::test]
    async fn split_read_extracts_udp_payload() {
        let (tx, rx) = tokio::sync::mpsc::channel::<Packet>(4);
        let mut reader = SplitRead { recv: rx };

        // Build a raw IPv4+UDP frame and inject it.
        let builder =
            PacketBuilder::ipv4([10, 0, 0, 1], [10, 0, 0, 2], 64).udp(2000, 53);
        let payload = b"query";
        let mut frame = Vec::with_capacity(builder.size(payload.len()));
        builder.write(&mut frame, payload).unwrap();

        tx.send(Packet::new(Bytes::from(frame))).await.unwrap();
        drop(tx);

        let received = reader.recv().await.expect("should parse a valid frame");
        assert_eq!(received.data(), payload);
        assert_eq!(received.src_addr.port(), 2000);
        assert_eq!(received.dst_addr.port(), 53);
    }

    #[tokio::test]
    async fn split_read_returns_none_on_garbage() {
        let (tx, rx) = tokio::sync::mpsc::channel::<Packet>(4);
        let mut reader = SplitRead { recv: rx };
        tx.send(Packet::new(Bytes::from_static(&[0xff, 0xff, 0xff]))).await.unwrap();
        let got = reader.recv().await;
        // Invalid IP packet → returns None (skipped, not propagated)
        assert!(got.is_none());
    }
}
