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

            let packet = match IpPacket::new_checked(&original_bytes) {
                Ok(p) => p,
                Err(_e) => {
                    error!("invalid IP packet: {_e}");
                    return None;
                }
            };

            let src_ip = packet.src_addr();
            let dst_ip = packet.dst_addr();
            let ip_payload = packet.payload();

            let udp_packet = match smoltcp::wire::UdpPacket::new_checked(ip_payload) {
                Ok(p) => p,
                Err(_e) => {
                    error!(
                        "invalid err: {_e}, src_ip: {src_ip}, dst_ip: {dst_ip}, \
                         payload: {ip_payload:?}"
                    );
                    return None;
                }
            };
            let src_port = udp_packet.src_port();
            let dst_port = udp_packet.dst_port();

            let udp_payload_slice = udp_packet.payload();

            let original_ptr = original_bytes.as_ptr() as usize;
            let payload_ptr = udp_payload_slice.as_ptr() as usize;

            let offset = payload_ptr - original_ptr;
            let len = udp_payload_slice.len();

            let payload_bytes = original_bytes.slice(offset..offset + len);

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
