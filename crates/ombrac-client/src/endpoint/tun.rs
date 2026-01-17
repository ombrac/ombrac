use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
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
use ombrac_transport::quic::Connection as QuicConnection;
use ombrac_transport::quic::client::Client as QuicClient;

pub use tun_rs::AsyncDevice;

pub use self::fakedns::FakeDns;
pub use crate::client::Client;

mod fakedns {
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    use crossbeam_queue::SegQueue;
    use hickory_proto::op::{Message, MessageType, ResponseCode};
    use hickory_proto::rr::{Name, RData, Record, RecordType};
    use ipnet::Ipv4Net;
    use moka::future::Cache;
    use moka::notification::ListenerFuture;
    use ombrac_macros::{debug, warn};

    const DNS_RESPONSE_TTL: u32 = 5;
    const CACHE_TTL: Duration = Duration::from_secs(DNS_RESPONSE_TTL as u64 + (7 * 24 * 60 * 60));

    #[derive(Clone)]
    pub struct FakeDns {
        ip_net: Ipv4Net,
        domain_to_ip: Cache<Name, Ipv4Addr>,
        ip_to_domain: Cache<Ipv4Addr, Name>,
        recycled_ips: Arc<SegQueue<Ipv4Addr>>,
        cursor: Arc<AtomicU32>,
        max_hosts: u32,
    }

    impl FakeDns {
        pub fn new(ip_net: Ipv4Net) -> Self {
            let max_hosts = ip_net.hosts().count() as u32;
            let recycled_ips = Arc::new(SegQueue::new());
            let cursor = Arc::new(AtomicU32::new(2));

            let domain_to_ip: Cache<Name, Ipv4Addr> = Cache::builder()
                .max_capacity(max_hosts as u64)
                .time_to_idle(CACHE_TTL)
                .build();

            let recycled_ips_ref = recycled_ips.clone();
            let domain_cache_ref = domain_to_ip.clone();

            let listener = move |k: Arc<Ipv4Addr>, v: Name, cause| -> ListenerFuture {
                let q = recycled_ips_ref.clone();
                let d_cache = domain_cache_ref.clone();
                Box::pin(async move {
                    d_cache.invalidate(&v).await;
                    q.push(*k);
                    debug!(ip = %k, domain = %v, reason = ?cause, "fakedns ip recycled");
                })
            };

            let ip_to_domain = Cache::builder()
                .max_capacity(max_hosts as u64)
                .time_to_idle(CACHE_TTL)
                .async_eviction_listener(listener)
                .build();

            Self {
                ip_net,
                domain_to_ip,
                ip_to_domain,
                recycled_ips,
                cursor,
                max_hosts,
            }
        }

        async fn allocate_unique_ip(&self) -> Option<Ipv4Addr> {
            if let Some(ip) = self.recycled_ips.pop() {
                return Some(ip);
            }

            loop {
                let current = self.cursor.load(Ordering::Relaxed);
                if current >= self.max_hosts {
                    break;
                }

                if self
                    .cursor
                    .compare_exchange(current, current + 1, Ordering::SeqCst, Ordering::Relaxed)
                    .is_ok()
                {
                    let network_u32 = u32::from(self.ip_net.network());
                    let new_ip = Ipv4Addr::from(network_u32 + 1 + current);
                    return Some(new_ip);
                }
            }

            warn!("fakedns ip pool is exhausted! no recycled or new ips available");
            None
        }

        pub async fn handle_dns_query(&self, query_bytes: &[u8]) -> Option<Message> {
            let query = match Message::from_vec(query_bytes) {
                Ok(q) => q,
                Err(e) => {
                    warn!(error = %e, "failed to parse dns query");
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
                let new_ip = self.allocate_unique_ip().await?;
                self.ip_to_domain.insert(new_ip, domain_name.clone()).await;
                self.domain_to_ip.insert(domain_name.clone(), new_ip).await;
                debug!(domain = %domain_name, fake_ip = %new_ip, "fakedns mapped new domain");
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
    pub disable_udp_443: bool,
}

impl Default for TunConfig {
    fn default() -> Self {
        Self {
            fakedns_cidr: "198.18.0.0/16"
                .parse()
                .expect("default fake dns cidr is invalid"),
            udp_idle_timeout: Duration::from_secs(60),
            udp_session_channel_capacity: 2048,
            disable_udp_443: false,
        }
    }
}

pub struct Tun {
    config: Arc<TunConfig>,
    client: Arc<Client<QuicClient, QuicConnection>>,
    fakedns: Arc<FakeDns>,
}

impl Clone for Tun {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            client: self.client.clone(),
            fakedns: self.fakedns.clone(),
        }
    }
}

impl Tun {
    pub fn new(config: Arc<TunConfig>, client: Arc<Client<QuicClient, QuicConnection>>) -> Self {
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
            tokio::spawn(Self::forward_packets_from_stack_to_tun(
                stack_stream,
                tun_sink,
                shutdown_token.clone(),
            )),
            tokio::spawn(Self::forward_packets_from_tun_to_stack(
                tun_stream,
                stack_sink,
                shutdown_token.clone(),
            )),
            tokio::spawn(
                self.clone()
                    .accept_tcp_connections(tcp_listener, shutdown_token.clone()),
            ),
            tokio::spawn(
                self.clone()
                    .process_incoming_udp_packets(udp_socket, shutdown_token.clone()),
            ),
        ];

        shutdown_signal.await;
        shutdown_token.cancel();

        for task in processing_tasks {
            if let Err(err) = task.await {
                error!(error = ?err, "processing task panicked or failed");
            }
        }

        Ok(())
    }

    async fn forward_packets_from_stack_to_tun(
        mut stack_stream: StackSplitStream,
        mut tun_sink: SplitSink<DeviceFramed<BytesCodec>, Bytes>,
        token: CancellationToken,
    ) {
        loop {
            tokio::select! {
                biased;
                _ = token.cancelled() => break,
                packet_result = stack_stream.next() => {
                    match packet_result {
                        Some(Ok(packet)) => {
                            if let Err(err) = tun_sink.send(packet.into_bytes()).await {
                                error!("failed to send packet to tun device: {}, stopping forwarding", err);
                                break;
                            }
                        }
                        Some(Err(err)) => {
                            error!("netstack read error: {}, stopping forwarding", err);
                            break;
                        }
                        None => break,
                    }
                }
            }
        }
        debug!("stack-to-tun forwarding finished");
    }

    async fn forward_packets_from_tun_to_stack(
        mut tun_stream: SplitStream<DeviceFramed<BytesCodec>>,
        mut stack_sink: StackSplitSink,
        token: CancellationToken,
    ) {
        loop {
            tokio::select! {
                biased;
                _ = token.cancelled() => break,
                packet_result = tun_stream.next() => {
                    match packet_result {
                        Some(Ok(packet)) => {
                            if let Err(err) = stack_sink.send(Packet::new(packet)).await {
                                error!("failed to send packet to stack: {}, stopping forwarding", err);
                                break;
                            }
                        }
                        Some(Err(err)) => {
                            error!("tun device read error: {}, stopping forwarding", err);
                            break;
                        }
                        None => {
                            info!("tun device stream closed, stopping tun-to-stack forwarding");
                            break;
                        }
                    }
                }
            }
        }
        debug!("tun-to-stack forwarding finished");
    }

    async fn accept_tcp_connections(
        self,
        mut tcp_listener: TcpConnection,
        token: CancellationToken,
    ) {
        loop {
            tokio::select! {
                biased;
                _ = token.cancelled() => break,
                stream_opt = tcp_listener.next() => {
                    let stream = match stream_opt {
                        Some(s) => s,
                        None => break,
                    };

                    let tun_instance = self.clone();
                    tokio::spawn(async move {
                        let remote_addr = stream.remote_addr();
                        if let Err(err) = tun_instance.relay_tcp_stream(stream).await {
                            error!(
                                src_addr = %remote_addr,
                                error = %err,
                                "tcp connect"
                            );
                        }
                    });
                }
            }
        }
        debug!("tcp connection acceptor finished");
    }

    async fn relay_tcp_stream(&self, mut stream: TcpStream) -> io::Result<()> {
        let local_addr = stream.local_addr();
        let fake_remote_addr = stream.remote_addr();

        let target_addr =
            if let Some(domain) = self.fakedns.get_domain_by_ip(&fake_remote_addr.ip()).await {
                Address::from((domain.to_utf8(), fake_remote_addr.port()))
            } else {
                if let SocketAddr::V4(addr) = fake_remote_addr
                    && self.config.fakedns_cidr.contains(addr.ip())
                {
                    return Err(io::Error::other(format!(
                        "dns cache miss: {} -> {}",
                        local_addr, fake_remote_addr
                    )));
                }
                Address::from(fake_remote_addr)
            };

        // Skip private/reserved addresses to avoid wasting resources
        if Self::should_skip_address(&target_addr) {
            return Err(io::Error::other(format!(
                "skipping private/reserved address: {} -> {}",
                local_addr, target_addr
            )));
        }

        let mut remote_stream = self.client.open_bidirectional(target_addr.clone()).await?;

        match ombrac_transport::io::copy_bidirectional(&mut stream, &mut remote_stream).await {
            Ok(stats) => {
                info!(
                    src_addr = %local_addr,
                    fake_addr = %fake_remote_addr,
                    dst_addr = %target_addr,
                    send = stats.a_to_b_bytes,
                    recv = stats.b_to_a_bytes,
                    "tcp connect"
                );
            }
            Err((err, stats)) => {
                error!(
                    src_addr = %local_addr,
                    fake_addr = %fake_remote_addr,
                    dst_addr = %target_addr,
                    send = stats.a_to_b_bytes,
                    recv = stats.b_to_a_bytes,
                    error = %err,
                    "tcp connect"
                );
                return Err(err);
            }
        }

        Ok(())
    }

    async fn process_incoming_udp_packets(self, udp_socket: UdpTunnel, token: CancellationToken) {
        let (mut reader, writer) = udp_socket.split();
        let active_sessions = Arc::new(DashMap::new());

        loop {
            tokio::select! {
                biased;
                _ = token.cancelled() => break,

                packet_opt = reader.recv() => {
                    let packet = match packet_opt {
                        Some(p) => p,
                        None => break,
                    };

                    let src_addr = packet.src_addr;
                    let dst_addr = packet.dst_addr;

                    if dst_addr.port() == 53 {
                        let mut dns_writer = writer.clone();
                        self.handle_dns_query_packet(packet, &mut dns_writer).await;
                        continue;
                    }

                    // Skip UDP packets to port 443 if disabled
                    if self.config.disable_udp_443 && dst_addr.port() == 443 {
                        info!(
                            src_addr = %src_addr,
                            dst_addr = %dst_addr,
                            "dropping udp packet to port 443"
                        );
                        continue;
                    }

                    if let Err(err) = self.handle_udp_data_packet(packet, &active_sessions, writer.clone()).await {
                        error!(
                            src_addr = %src_addr,
                            dst_addr = %dst_addr,
                            error = %err,
                            "udp error"
                        );
                    };
                }
            }
        }
        debug!("udp packet processing finished");
    }

    async fn handle_dns_query_packet(&self, packet: UdpPacket, writer: &mut SplitWrite) {
        let query_data: Bytes = packet.data.into_bytes();
        if let Some(response_message) = self.fakedns.handle_dns_query(&query_data).await {
            match response_message.to_vec() {
                Ok(response_bytes) => {
                    let response_packet = UdpPacket {
                        data: Packet::new(response_bytes),
                        src_addr: packet.dst_addr,
                        dst_addr: packet.src_addr,
                    };
                    if let Err(err) = writer.send(response_packet).await {
                        error!(error = %err, "failed to send dns response to tun stack");
                    }
                }
                Err(err) => {
                    error!(error = %err, "failed to serialize dns response");
                }
            }
        }
    }

    async fn handle_udp_data_packet(
        &self,
        packet: UdpPacket,
        active_sessions: &Arc<DashMap<SocketAddr, mpsc::Sender<(Bytes, Address)>>>,
        writer: SplitWrite,
    ) -> io::Result<()> {
        let local_addr = packet.src_addr;
        let fake_remote_addr = packet.dst_addr;
        let packet_data = packet.data.into_bytes();

        let target_addr =
            if let Some(domain) = self.fakedns.get_domain_by_ip(&fake_remote_addr.ip()).await {
                Address::from((domain.to_utf8(), fake_remote_addr.port()))
            } else {
                if let SocketAddr::V4(addr) = fake_remote_addr
                    && self.config.fakedns_cidr.contains(addr.ip())
                {
                    return Err(io::Error::other(format!(
                        "dns cache miss: {} -> {}",
                        local_addr, fake_remote_addr
                    )));
                }
                Address::from(fake_remote_addr)
            };

        // Skip private/reserved addresses to avoid wasting resources
        if Self::should_skip_address(&target_addr) {
            return Err(io::Error::other(format!(
                "skipping private/reserved address: {} -> {}",
                local_addr, target_addr
            )));
        }

        match active_sessions.entry(local_addr) {
            Entry::Occupied(entry) => {
                if entry.get().send((packet_data, target_addr)).await.is_err() {
                    entry.remove();
                }
            }
            Entry::Vacant(entry) => {
                let (sender, receiver) = mpsc::channel(self.config.udp_session_channel_capacity);
                if sender
                    .send((packet_data, target_addr.clone()))
                    .await
                    .is_err()
                {
                    return Ok(());
                }

                entry.insert(sender);

                #[cfg(feature = "datagram")]
                tokio::spawn(self.clone().relay_udp_flow(
                    receiver,
                    writer,
                    active_sessions.clone(),
                    local_addr,
                    fake_remote_addr,
                ));
            }
        }

        Ok(())
    }

    #[cfg(feature = "datagram")]
    async fn relay_udp_flow(
        self,
        mut receiver: mpsc::Receiver<(Bytes, Address)>,
        mut writer: SplitWrite,
        active_sessions: Arc<DashMap<SocketAddr, mpsc::Sender<(Bytes, Address)>>>,
        local_addr: SocketAddr,
        fake_remote_addr: SocketAddr,
    ) {
        let mut udp_session = self.client.open_associate();

        let idle_timeout = tokio::time::sleep(self.config.udp_idle_timeout);
        tokio::pin!(idle_timeout);

        let mut total_send_bytes = 0u64;
        let mut total_recv_bytes = 0u64;
        let mut target_addr: Option<Address> = None;

        loop {
            tokio::select! {
                Some((packet_data, addr)) = receiver.recv() => {
                    if target_addr.is_none() {
                        target_addr = Some(addr.clone());
                    }
                    total_send_bytes += packet_data.len() as u64;
                    if let Err(err) = udp_session.send_to(packet_data, addr.clone()).await {
                        error!(
                            local_addr = %local_addr,
                            target = %addr,
                            error = %err,
                            "udp send failed"
                        );
                    }
                    idle_timeout.as_mut().reset(tokio::time::Instant::now() + self.config.udp_idle_timeout);
                }

                Some((packet_data, _source_addr)) = udp_session.recv_from() => {
                    total_recv_bytes += packet_data.len() as u64;
                    let response_packet = UdpPacket {
                        data: Packet::new(packet_data),
                        src_addr: fake_remote_addr,
                        dst_addr: local_addr,
                    };

                    if writer.send(response_packet).await.is_err() {
                        error!(
                            local_addr = %local_addr,
                            "udp send to tun stack failed, terminating flow"
                        );
                        break;
                    }
                    idle_timeout.as_mut().reset(tokio::time::Instant::now() + self.config.udp_idle_timeout);
                }

                _ = &mut idle_timeout => {
                    debug!(
                        local_addr = %local_addr,
                        fake_remote_addr = %fake_remote_addr,
                        idle_timeout_secs = self.config.udp_idle_timeout.as_secs(),
                        "udp session timeout"
                    );
                    break;
                }
            }
        }

        active_sessions.remove(&local_addr);
        if let Some(target) = target_addr {
            info!(
                src_addr = %local_addr,
                fake_addr = %fake_remote_addr,
                dst_addr = %target,
                send = total_send_bytes,
                recv = total_recv_bytes,
                "udp session"
            );
        } else {
            info!(
                src_addr = %local_addr,
                fake_addr = %fake_remote_addr,
                send = total_send_bytes,
                recv = total_recv_bytes,
                "udp session"
            );
        }
    }

    /// Check if an IP address is a private/local network or reserved address.
    /// Returns true if the address should be skipped (not tunneled).
    fn is_private_or_reserved(ip: &IpAddr) -> bool {
        match ip {
            IpAddr::V4(v4) => {
                v4.is_private()
                    || v4.is_loopback()
                    || v4.is_link_local()
                    || v4.is_multicast()
                    || v4.is_broadcast()
                    || v4.is_documentation()
                    || match v4.octets() {
                        [0, ..] => true,                             // Current network
                        [100, b, ..] if b >= 64 && b <= 127 => true, // CGNAT
                        [192, 0, 0, ..] => true,                     // IETF Protocol Assignments
                        [198, 18, ..] | [198, 19, ..] => true,       // Benchmarking
                        [240, ..] => true,                           // Reserved/Experimental
                        _ => false,
                    }
            }
            IpAddr::V6(v6) => {
                v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_multicast()
                    || (v6.segments()[0] & 0xfe00) == 0xfc00
                    || (v6.segments()[0] & 0xffc0) == 0xfe80
                    || v6.segments()[0] == 0x2001 && v6.segments()[1] == 0xdb8
                    || v6.segments()[0] == 0x100
            }
        }
    }

    fn should_skip_address(addr: &Address) -> bool {
        match addr {
            Address::SocketV4(sock) => Self::is_private_or_reserved(&IpAddr::V4(*sock.ip())),
            Address::SocketV6(sock) => Self::is_private_or_reserved(&IpAddr::V6(*sock.ip())),
            Address::Domain(_, _) => false, // Domain names are resolved later, skip check here
        }
    }
}
