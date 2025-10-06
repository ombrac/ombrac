use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use smoltcp::wire::{IpProtocol, IpVersion, Ipv4Address, Ipv4Packet, Ipv6Address, Ipv6Packet};
use tokio::sync::mpsc;

use crate::tcp::TcpConnection;
use crate::{buffer::BufferPool, udp::UdpTunnel};
use crate::{debug, error};

const DEFAULT_IPV4_ADDR: Ipv4Address = Ipv4Address::new(10, 0, 0, 1);
const DEFAULT_IPV6_ADDR: Ipv6Address = Ipv6Address::new(0x0, 0xfac, 0, 0, 0, 0, 0, 1);

#[derive(Debug)]
pub enum IpPacket<T: AsRef<[u8]>> {
    Ipv4(Ipv4Packet<T>),
    Ipv6(Ipv6Packet<T>),
}

impl<T: AsRef<[u8]> + Copy> IpPacket<T> {
    pub fn new_checked(packet: T) -> smoltcp::wire::Result<IpPacket<T>> {
        let buffer = packet.as_ref();
        match IpVersion::of_packet(buffer)? {
            IpVersion::Ipv4 => Ok(IpPacket::Ipv4(Ipv4Packet::new_checked(packet)?)),
            IpVersion::Ipv6 => Ok(IpPacket::Ipv6(Ipv6Packet::new_checked(packet)?)),
        }
    }

    pub fn src_addr(&self) -> IpAddr {
        match *self {
            IpPacket::Ipv4(ref packet) => IpAddr::from(packet.src_addr()),
            IpPacket::Ipv6(ref packet) => IpAddr::from(packet.src_addr()),
        }
    }

    pub fn dst_addr(&self) -> IpAddr {
        match *self {
            IpPacket::Ipv4(ref packet) => IpAddr::from(packet.dst_addr()),
            IpPacket::Ipv6(ref packet) => IpAddr::from(packet.dst_addr()),
        }
    }

    pub fn protocol(&self) -> IpProtocol {
        match *self {
            IpPacket::Ipv4(ref packet) => packet.next_header(),
            IpPacket::Ipv6(ref packet) => packet.next_header(),
        }
    }
}

impl<'a, T: AsRef<[u8]> + ?Sized> IpPacket<&'a T> {
    #[inline]
    pub fn payload(&self) -> &'a [u8] {
        match *self {
            IpPacket::Ipv4(ref packet) => packet.payload(),
            IpPacket::Ipv6(ref packet) => packet.payload(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct NetStackConfig {
    pub mtu: usize,
    pub channel_size: usize,
    pub number_workers: usize,

    pub tcp_send_buffer_size: usize,
    pub tcp_recv_buffer_size: usize,

    pub buffer_pool_size: usize,
    pub buffer_pool_default_buffer_size: usize,

    pub ipv4_addr: Ipv4Addr,
    pub ipv4_prefix_len: u8,
    pub ipv6_addr: Ipv6Addr,
    pub ipv6_prefix_len: u8,

    pub tcp_keep_alive: Duration,
    pub tcp_timeout: Duration,
    pub packet_batch_size: usize,
    pub ip_ttl: u8,
}

impl Default for NetStackConfig {
    fn default() -> Self {
        Self {
            mtu: 1500,
            channel_size: 4096,
            number_workers: std::thread::available_parallelism().map_or(4, |n| n.get()),
            tcp_send_buffer_size: 16 * 1024,
            tcp_recv_buffer_size: 16 * 1024,
            buffer_pool_size: 32,
            buffer_pool_default_buffer_size: 2 * 1024,
            ipv4_addr: DEFAULT_IPV4_ADDR,
            ipv4_prefix_len: 24,
            ipv6_addr: DEFAULT_IPV6_ADDR,
            ipv6_prefix_len: 64,
            tcp_timeout: Duration::from_secs(60),
            tcp_keep_alive: Duration::from_secs(28),
            packet_batch_size: 32,
            ip_ttl: 64,
        }
    }
}

pub struct NetStack {
    udp_inbound: mpsc::Sender<Packet>,
    tcp_inbound: mpsc::Sender<Packet>,
    packet_outbound: mpsc::Receiver<Packet>,
}

pub struct Packet {
    data: Bytes,
}

impl Packet {
    pub fn new(data: impl Into<Bytes>) -> Self {
        Packet { data: data.into() }
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn into_bytes(self) -> Bytes {
        self.data
    }
}

impl<T> From<T> for Packet
where
    T: Into<Bytes>,
{
    fn from(data: T) -> Self {
        Packet::new(data)
    }
}

impl NetStack {
    pub fn new(config: NetStackConfig) -> (Self, TcpConnection, UdpTunnel) {
        let (packet_sender, packet_receiver) = mpsc::channel::<Packet>(config.channel_size);
        let (udp_inbound_app, udp_outbound_stack) = mpsc::channel::<Packet>(config.channel_size);
        let (tcp_inbound_app, tcp_outbound_stack) = mpsc::channel::<Packet>(config.channel_size);
        let buffer_pool = Arc::new(BufferPool::new(
            config.buffer_pool_size,
            config.buffer_pool_default_buffer_size,
        ));

        (
            NetStack {
                udp_inbound: udp_inbound_app,
                tcp_inbound: tcp_inbound_app,
                packet_outbound: packet_receiver,
            },
            TcpConnection::new(
                config.clone(),
                tcp_outbound_stack,
                packet_sender.clone(),
                buffer_pool.clone(),
            ),
            UdpTunnel::new(
                config.into(),
                udp_outbound_stack,
                packet_sender.clone(),
                buffer_pool.clone(),
            ),
        )
    }

    pub fn split(self) -> (StackSplitSink, StackSplitStream) {
        (
            StackSplitSink::new(self.udp_inbound, self.tcp_inbound),
            StackSplitStream::new(self.packet_outbound),
        )
    }
}

pub struct StackSplitSink {
    udp_inbound: mpsc::Sender<Packet>,
    tcp_inbound: mpsc::Sender<Packet>,
    packet_container: Option<(Packet, IpProtocol)>,
}

impl StackSplitSink {
    pub fn new(udp_inbound: mpsc::Sender<Packet>, tcp_inbound: mpsc::Sender<Packet>) -> Self {
        Self {
            udp_inbound,
            tcp_inbound,
            packet_container: None,
        }
    }
}

impl futures::Sink<Packet> for StackSplitSink {
    type Error = io::Error;

    fn poll_ready(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        if self.packet_container.is_some() {
            match self.as_mut().poll_flush(cx) {
                Poll::Ready(Ok(())) => {}
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            }
        }
        Poll::Ready(Ok(()))
    }

    fn start_send(mut self: std::pin::Pin<&mut Self>, item: Packet) -> Result<(), Self::Error> {
        if item.data().is_empty() {
            return Ok(());
        }

        let packet = IpPacket::new_checked(item.data())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        let protocol = packet.protocol();
        if matches!(
            protocol,
            IpProtocol::Tcp | IpProtocol::Udp | IpProtocol::Icmp | IpProtocol::Icmpv6
        ) {
            self.packet_container.replace((item, protocol));
        } else {
            error!("IP packet ignored protocol: {protocol:?}");
        }

        Ok(())
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        let (item, proto) = match self.packet_container.take() {
            Some(val) => val,
            None => return Poll::Ready(Ok(())),
        };

        let sender = match proto {
            IpProtocol::Udp => self.udp_inbound.clone(),
            IpProtocol::Tcp | IpProtocol::Icmp | IpProtocol::Icmpv6 => self.tcp_inbound.clone(),
            _ => {
                error!("Unsupported protocol for packet: {proto:?}");
                return Poll::Ready(Ok(()));
            }
        };
        let mut fut = Box::pin(sender.reserve());

        match fut.as_mut().poll(cx) {
            Poll::Ready(Ok(permit)) => {
                permit.send(item);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(_)) => {
                let msg = format!("Failed to send packet: channel closed for protocol {proto:?}");
                debug!("{}", msg);
                Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, msg)))
            }
            Poll::Pending => {
                self.packet_container = Some((item, proto));
                Poll::Pending
            }
        }
    }

    fn poll_close(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}

pub struct StackSplitStream {
    packet_outbound: mpsc::Receiver<Packet>,
}

impl StackSplitStream {
    pub fn new(packet_outbound: mpsc::Receiver<Packet>) -> Self {
        Self { packet_outbound }
    }
}

impl futures::Stream for StackSplitStream {
    type Item = io::Result<Packet>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        match self.packet_outbound.poll_recv(cx) {
            Poll::Ready(Some(packet)) => Poll::Ready(Some(Ok(packet))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}
