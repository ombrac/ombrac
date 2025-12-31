use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use dashmap::{DashMap, Entry};
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use ipnet::Ipv4Net;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tun_rs::async_framed::{BytesCodec, DeviceFramed};

use ombrac::protocol::Address;
use ombrac_macros::{debug, error, info};
use ombrac_netstack::{
    stack::{NetStack, NetStackConfig, Packet, StackSplitSink, StackSplitStream},
    tcp::{TcpConnection, TcpStream},
    udp::{SplitWrite, UdpPacket, UdpTunnel},
};
use ombrac_transport::{Connection, Initiator};

pub use tun_rs::AsyncDevice;

pub use self::fakedns::FakeDns;
pub use crate::client::Client;

mod fakedns {
    use std::hash::{Hash, Hasher};
    use std::net::{IpAddr, Ipv4Addr};
    use std::time::Duration;

    use hickory_proto::op::{Message, MessageType, ResponseCode};
    use hickory_proto::rr::{Name, RData, Record, RecordType};
    use ipnet::Ipv4Net;
    use moka::future::Cache;
    use ombrac_macros::{debug, warn};
    use twox_hash::XxHash64;

    const XX_HASH_SEED: u64 = 0;
    const DNS_RESPONSE_TTL: u32 = 5;
    const CACHE_TTL: Duration = Duration::from_secs(DNS_RESPONSE_TTL as u64 + (7 * 24 * 60 * 60));

    #[derive(Clone)]
    pub struct FakeDns {
        ip_net: Ipv4Net,
        domain_to_ip: Cache<Name, Ipv4Addr>,
        ip_to_domain: Cache<Ipv4Addr, Name>,
    }

    impl FakeDns {
        pub fn new(ip_net: Ipv4Net) -> Self {
            Self {
                ip_net,
                domain_to_ip: Cache::builder()
                    .max_capacity(10_000)
                    .time_to_idle(CACHE_TTL)
                    .build(),
                ip_to_domain: Cache::builder()
                    .max_capacity(10_000)
                    .time_to_idle(CACHE_TTL)
                    .build(),
            }
        }

        fn get_ip_for_domain(&self, domain_name: &Name) -> Option<Ipv4Addr> {
            let num_hosts = self.ip_net.hosts().count() as u64;
            if num_hosts == 0 {
                return None;
            }

            let network_addr = self.ip_net.network();
            let mut hasher = XxHash64::with_seed(XX_HASH_SEED);
            domain_name.hash(&mut hasher);
            let hash_val = hasher.finish();
            let offset = (hash_val % num_hosts) + 1;
            let ip_as_u32 = u32::from(network_addr).wrapping_add(offset as u32);
            Some(Ipv4Addr::from(ip_as_u32))
        }

        pub async fn handle_dns_query(&self, query_bytes: &[u8]) -> Option<Message> {
            let query = match Message::from_vec(query_bytes) {
                Ok(q) => q,
                Err(e) => {
                    warn!("Failed to parse DNS query: {}", e);
                    return None;
                }
            };

            if query.queries().is_empty() {
                return None;
            }

            let question = &query.queries()[0];
            let domain_name = question.name();

            if question.query_type() != RecordType::A {
                let mut response = query.clone();
                response.set_message_type(MessageType::Response);
                response.set_response_code(ResponseCode::Refused);
                return Some(response);
            }

            let fake_ip = if let Some(ip) = self.domain_to_ip.get(domain_name).await {
                ip
            } else {
                let new_ip = self.get_ip_for_domain(domain_name)?;
                self.domain_to_ip.insert(domain_name.clone(), new_ip).await;
                self.ip_to_domain.insert(new_ip, domain_name.clone()).await;
                debug!(
                    "FakeDNS: Mapped {} -> {} (Stateless Calculation)",
                    domain_name, new_ip
                );
                new_ip
            };

            let mut response = query.clone();
            response.set_message_type(MessageType::Response);
            response.set_response_code(ResponseCode::NoError);
            let record = Record::from_rdata(
                domain_name.clone(),
                DNS_RESPONSE_TTL,
                RData::A(hickory_proto::rr::rdata::A(fake_ip)),
            );
            response.add_answer(record);

            Some(response)
        }

        pub async fn get_domain_by_ip(&self, ip: &IpAddr) -> Option<Name> {
            if let IpAddr::V4(ipv4) = ip {
                self.ip_to_domain.get(ipv4).await
            } else {
                None
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct TunConfig {
    pub fakedns_cidr: Ipv4Net,
    pub udp_idle_timeout: Duration,
    pub udp_session_channel_capacity: usize,
}

impl Default for TunConfig {
    fn default() -> Self {
        Self {
            fakedns_cidr: "198.18.0.0/16"
                .parse()
                .expect("Hardcoded FakeDNS CIDR is invalid"),
            udp_idle_timeout: Duration::from_secs(60),
            udp_session_channel_capacity: 128,
        }
    }
}

pub struct Tun<T, C>
where
    T: Initiator<Connection = C>,
    C: Connection,
{
    config: Arc<TunConfig>,
    client: Arc<Client<T, C>>,
    fakedns: Arc<FakeDns>,
}

impl<T, C> Clone for Tun<T, C>
where
    T: Initiator<Connection = C>,
    C: Connection,
{
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            client: self.client.clone(),
            fakedns: self.fakedns.clone(),
        }
    }
}

impl<T, C> Tun<T, C>
where
    T: Initiator<Connection = C>,
    C: Connection,
{
    pub fn new(config: Arc<TunConfig>, client: Arc<Client<T, C>>) -> Self {
        Self {
            fakedns: Arc::new(FakeDns::new(config.fakedns_cidr)),
            config,
            client,
        }
    }

    pub async fn accept_loop(
        &self,
        device: AsyncDevice,
        shutdown_signal: impl Future<Output = ()>,
    ) -> std::io::Result<()> {
        let framed = DeviceFramed::new(device, BytesCodec::new());
        let (tun_sink, tun_stream) = framed.split::<bytes::Bytes>();

        let (stack, tcp_listener, udp_socket) = NetStack::new(NetStackConfig::default());
        let (stack_sink, stack_stream) = stack.split();

        let shutdown_token = CancellationToken::new();

        let processing_tasks = vec![
            tokio::spawn(Self::process_stack_to_tun(
                stack_stream,
                tun_sink,
                shutdown_token.clone(),
            )),
            tokio::spawn(Self::process_tun_to_stack(
                tun_stream,
                stack_sink,
                shutdown_token.clone(),
            )),
            tokio::spawn(
                self.clone()
                    .process_tcp_connections(tcp_listener, shutdown_token.clone()),
            ),
            tokio::spawn(
                self.clone()
                    .process_udp_packets(udp_socket, shutdown_token.clone()),
            ),
        ];

        shutdown_signal.await;
        shutdown_token.cancel();

        for task in processing_tasks {
            if let Err(err) = task.await {
                error!("A processing task panicked or failed: {:?}", err);
            }
        }

        debug!("TUN stack has shut down completely.");
        Ok(())
    }

    async fn process_stack_to_tun(
        mut stack_stream: StackSplitStream,
        mut tun_sink: SplitSink<DeviceFramed<BytesCodec>, Bytes>,
        token: CancellationToken,
    ) {
        loop {
            tokio::select! {
                biased;
                _ = token.cancelled() => break,
                pkt_result = stack_stream.next() => {
                    match pkt_result {
                        Some(Ok(pkt)) => {
                            if let Err(err) = tun_sink.send(pkt.into_bytes()).await {
                                error!("Failed to send packet to TUN: {}. Stopping task.", err);
                                break;
                            }
                        }
                        Some(Err(err)) => {
                            error!("Netstack read error: {}. Stopping task.", err);
                            break;
                        }
                        None => break,
                    }
                }
            }
        }
        debug!("Stack-to-TUN task has finished.");
    }

    async fn process_tun_to_stack(
        mut tun_stream: SplitStream<DeviceFramed<BytesCodec>>,
        mut stack_sink: StackSplitSink,
        token: CancellationToken,
    ) {
        loop {
            tokio::select! {
                biased;
                _ = token.cancelled() => break,
                pkt_result = tun_stream.next() => {
                    match pkt_result {
                        Some(Ok(pkt)) => {
                            if let Err(err) = stack_sink.send(Packet::new(pkt)).await {
                                error!("Failed to send packet to stack: {}. Stopping task.", err);
                                break;
                            }
                        }
                        Some(Err(err)) => {
                            error!("TUN stream read error: {}. Stopping task.", err);
                            break;
                        }
                        None => {
                            info!("TUN stream closed. Stopping TUN-to-stack task.");
                            break;
                        }
                    }
                }
            }
        }
        debug!("TUN-to-stack task has finished.");
    }

    async fn process_tcp_connections(
        self,
        mut tcp_listener: TcpConnection,
        token: CancellationToken,
    ) {
        loop {
            tokio::select! {
                biased;
                _ = token.cancelled() => break,
                stream_option = tcp_listener.next() => {
                    let stream = match stream_option {
                        Some(s) => s,
                        None => break,
                    };

                    let self_clone = self.clone();
                    tokio::spawn(async move {
                        let addr = stream.remote_addr();
                        if let Err(err) = self_clone.handle_tcp_stream(stream).await {
                            error!("Error handling {} TCP stream: {}", addr, err);
                        }
                    });
                }
            }
        }
        debug!("TCP connection processing task has finished.");
    }

    async fn handle_tcp_stream(&self, mut stream: TcpStream) -> io::Result<()> {
        let local_addr = stream.local_addr();
        let remote_addr = stream.remote_addr();

        let target_addr =
            if let Some(domain) = self.fakedns.get_domain_by_ip(&remote_addr.ip()).await {
                Address::from((domain.to_utf8(), remote_addr.port()))
            } else {
                if let SocketAddr::V4(addr) = remote_addr
                    && self.config.fakedns_cidr.contains(addr.ip())
                {
                    return Err(io::Error::other(format!(
                        "DNS Cache Miss: {} -> {}",
                        local_addr, remote_addr
                    )));
                }

                Address::from(remote_addr)
            };

        let mut dest_stream = self.client.open_bidirectional(target_addr.clone()).await?;

        match ombrac_transport::io::copy_bidirectional(&mut stream, &mut dest_stream).await {
            Ok(stats) => {
                #[cfg(feature = "tracing")]
                tracing::info!(
                    src_addr = local_addr.to_string(),
                    fake_addr = remote_addr.to_string(),
                    dst_addr = target_addr.to_string(),
                    send = stats.a_to_b_bytes,
                    recv = stats.b_to_a_bytes,
                    status = "ok",
                    "Connect"
                );
            }
            Err((err, stats)) => {
                #[cfg(feature = "tracing")]
                tracing::error!(
                    src_addr = local_addr.to_string(),
                    fake_addr = remote_addr.to_string(),
                    dst_addr = target_addr.to_string(),
                    send = stats.a_to_b_bytes,
                    recv = stats.b_to_a_bytes,
                    status = "err",
                    error = %err,
                    "Connect"
                );
                return Err(err);
            }
        }

        Ok(())
    }

    async fn process_udp_packets(self, udp_socket: UdpTunnel, token: CancellationToken) {
        let (mut reader, writer) = udp_socket.split();
        let sessions: Arc<DashMap<SocketAddr, mpsc::Sender<(Bytes, Address)>>> =
            Arc::new(DashMap::new());

        loop {
            tokio::select! {
                biased;
                _ = token.cancelled() => break,

                packet_option = reader.recv() => {
                    let packet = match packet_option {
                        Some(p) => p,
                        None => break,
                    };

                    if packet.dst_addr.port() == 53 {
                        let mut dns_writer = writer.clone();
                        self.handle_dns_packet(packet, &mut dns_writer).await;
                        continue;
                    }

                    self.handle_data_packet(packet, &sessions, writer.clone()).await;
                }
            }
        }
        debug!("UDP packet processing task has finished.");
    }

    async fn handle_dns_packet(&self, packet: UdpPacket, writer: &mut SplitWrite) {
        let data: Bytes = packet.data.into_bytes();
        if let Some(response_message) = self.fakedns.handle_dns_query(&data).await {
            match response_message.to_vec() {
                Ok(response_bytes) => {
                    let dns_response_packet = UdpPacket {
                        data: Packet::new(response_bytes),
                        src_addr: packet.dst_addr,
                        dst_addr: packet.src_addr,
                    };
                    if let Err(e) = writer.send(dns_response_packet).await {
                        error!("Failed to send DNS response to TUN stack: {}", e);
                    }
                }
                Err(e) => {
                    error!("Failed to serialize DNS response: {}", e);
                }
            }
        }
    }

    async fn handle_data_packet(
        &self,
        packet: UdpPacket,
        sessions: &Arc<DashMap<SocketAddr, mpsc::Sender<(Bytes, Address)>>>,
        writer: SplitWrite,
    ) {
        let local_addr = packet.src_addr;
        let initial_remote_addr = packet.dst_addr;
        let data: Bytes = packet.data.into_bytes();

        let remote_addr: Address = if let Some(domain) = self
            .fakedns
            .get_domain_by_ip(&initial_remote_addr.ip())
            .await
        {
            Address::from((domain.to_utf8(), initial_remote_addr.port()))
        } else {
            Address::from(initial_remote_addr)
        };

        match sessions.entry(local_addr) {
            Entry::Occupied(entry) => {
                if entry.get().send((data, remote_addr)).await.is_err() {
                    entry.remove();
                }
            }
            Entry::Vacant(entry) => {
                let (tx, rx) = mpsc::channel(self.config.udp_session_channel_capacity);
                if tx.send((data, remote_addr.clone())).await.is_err() {
                    return;
                }

                entry.insert(tx);

                info!(
                    "[TUN] New UDP flow for local_addr={}, initial_remote_addr={}",
                    local_addr, initial_remote_addr
                );

                tokio::spawn(self.clone().handle_udp_flow(
                    rx,
                    writer,
                    sessions.clone(),
                    local_addr,
                    initial_remote_addr,
                ));
            }
        }
    }

    async fn handle_udp_flow(
        self,
        mut rx: mpsc::Receiver<(Bytes, Address)>,
        mut writer: SplitWrite,
        sessions: Arc<DashMap<SocketAddr, mpsc::Sender<(Bytes, Address)>>>,
        local_addr: SocketAddr,
        initial_remote_addr: SocketAddr,
    ) {
        debug!("UDP New Stream from {}", local_addr);
        let mut udp_session = self.client.open_associate();

        let idle_timeout = tokio::time::sleep(self.config.udp_idle_timeout);
        tokio::pin!(idle_timeout);

        loop {
            tokio::select! {
                Some((data, dest_addr)) = rx.recv() => {
                    info!("[TUN] UDP Upstream: local_addr={}, dest_addr={}, size={}", local_addr, dest_addr, data.len());
                    if let Err(e) = udp_session.send_to(data, dest_addr.clone()).await {
                        error!("Failed to send UDP packet from {} to {}: {}", local_addr, dest_addr, e);
                    }
                    idle_timeout.as_mut().reset(tokio::time::Instant::now() + self.config.udp_idle_timeout);
                }

                Some((data, from_addr)) = udp_session.recv_from() => {
                    info!("[TUN] UDP Downstream: from_addr={}, local_addr={}, size={}", from_addr, local_addr, data.len());
                    let response_packet = UdpPacket {
                        data: Packet::new(data),
                        src_addr: initial_remote_addr,
                        dst_addr: local_addr,
                    };

                    if writer.send(response_packet).await.is_err() {
                        error!("Failed to send UDP packet to TUN stack, terminating flow for {}", local_addr);
                        break;
                    }
                    idle_timeout.as_mut().reset(tokio::time::Instant::now() + self.config.udp_idle_timeout);
                }

                _ = &mut idle_timeout => {
                    info!("UDP stream {} to {} timed out due to inactivity.", local_addr, initial_remote_addr);
                    break;
                }
            }
        }

        sessions.remove(&local_addr);
        info!("UDP stream from {} closed and cleaned up.", local_addr);
    }
}
