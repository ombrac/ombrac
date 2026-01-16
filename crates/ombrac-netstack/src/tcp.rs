use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Observer, Producer};
use smoltcp::iface::{Interface, PollResult, SocketSet};
use smoltcp::socket::tcp::{CongestionControl, Socket, SocketBuffer, State};
use smoltcp::wire::{IpCidr, IpProtocol, TcpPacket};
use tokio::sync::{Notify, broadcast, mpsc};
use tokio::task::JoinHandle;

use crate::buffer::BufferPool;
use crate::device::NetstackDevice;
use crate::stack::{IpPacket, NetStackConfig, Packet};
use crate::{debug, error};

pub use stream::TcpStream;
pub(crate) use stream::{RbConsumer, RbProducer, SharedState};

struct TcpConnectionWorker {
    config: Arc<NetStackConfig>,
    device_injector: mpsc::Sender<Packet>,
    iface: Interface,
    sockets: SocketSet<'static>,
    socket_maps: HashMap<smoltcp::iface::SocketHandle, SocketIOHandle>,
    inbound: mpsc::Receiver<Packet>,
    socket_stream_emitter: mpsc::Sender<TcpStream>,
    notifier: Arc<Notify>,
    shutdown_rx: broadcast::Receiver<()>,
}

pub(crate) struct SocketIOHandle {
    recv_buffer_prod: RbProducer,
    send_buffer_cons: RbConsumer,
    shared_state: Arc<SharedState>,
}

pub struct TcpConnection {
    socket_stream: mpsc::Receiver<TcpStream>,
    shutdown_tx: broadcast::Sender<()>,
    _handles: Vec<JoinHandle<()>>,
}

impl Drop for TcpConnection {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(());
    }
}

impl TcpConnection {
    pub fn new(
        config: NetStackConfig,
        inbound: mpsc::Receiver<Packet>,
        outbound: mpsc::Sender<Packet>,
        buffer_pool: Arc<BufferPool>,
    ) -> Self {
        let num_workers = config.number_workers;
        let config = Arc::new(config);

        let (aggregated_socket_stream_emitter, aggregated_socket_stream_receiver) =
            mpsc::channel::<TcpStream>(config.channel_size);

        let (shutdown_tx, _) = broadcast::channel(1);

        let mut _handles = Vec::new();
        let mut worker_senders = Vec::with_capacity(num_workers);

        for _i in 0..num_workers {
            let (worker_inbound_sender, worker_inbound_receiver) =
                mpsc::channel(config.channel_size);
            worker_senders.push(worker_inbound_sender);

            let mut device = NetstackDevice::new(outbound.clone(), buffer_pool.clone(), &config);
            let device_injector = device.create_injector();
            let iface = Self::create_interface(&config, &mut device);
            let notifier = Arc::new(Notify::new());
            let shutdown_rx = shutdown_tx.subscribe();

            let mut worker = TcpConnectionWorker {
                config: config.clone(),
                device_injector,
                iface,
                sockets: SocketSet::new(vec![]),
                socket_maps: HashMap::new(),
                inbound: worker_inbound_receiver,
                socket_stream_emitter: aggregated_socket_stream_emitter.clone(),
                notifier: notifier.clone(),
                shutdown_rx,
            };

            let worker_handle = tokio::spawn(async move {
                if let Err(_e) = worker.accept_loop(device).await {
                    error!("[Worker {}] exited with error: {}", _i, _e);
                }
            });
            _handles.push(worker_handle);
        }

        let dispatcher_shutdown_rx = shutdown_tx.subscribe();
        let dispatcher_handle = tokio::spawn(Self::distribute_packets(
            inbound,
            worker_senders,
            dispatcher_shutdown_rx,
        ));
        _handles.push(dispatcher_handle);

        TcpConnection {
            socket_stream: aggregated_socket_stream_receiver,
            shutdown_tx,
            _handles,
        }
    }

    async fn distribute_packets(
        mut inbound: mpsc::Receiver<Packet>,
        worker_senders: Vec<mpsc::Sender<Packet>>,
        mut shutdown_rx: broadcast::Receiver<()>,
    ) {
        let num_workers = worker_senders.len();
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    debug!("[Dispatcher] received shutdown signal, exiting.");
                    break;
                }
                maybe_packet = inbound.recv() => {
                    if let Some(packet) = maybe_packet {
                        let worker_index = match IpPacket::new_checked(packet.data()) {
                            Ok(ip_packet) => {
                                if ip_packet.protocol() == IpProtocol::Tcp {
                                    if let Ok(tcp_packet) = TcpPacket::new_checked(ip_packet.payload()) {
                                        let mut addr1 =
                                            SocketAddr::new(ip_packet.src_addr(), tcp_packet.src_port());
                                        let mut addr2 =
                                            SocketAddr::new(ip_packet.dst_addr(), tcp_packet.dst_port());
                                        if addr1 > addr2 {
                                            std::mem::swap(&mut addr1, &mut addr2);
                                        }
                                        let mut hasher = DefaultHasher::new();
                                        addr1.hash(&mut hasher);
                                        addr2.hash(&mut hasher);
                                        (hasher.finish() % num_workers as u64) as usize
                                    } else { 0 }
                                } else { 0 }
                            }
                            Err(_) => 0,
                        };

                        if worker_senders[worker_index].send(packet).await.is_err() {
                            error!(
                                "[Dispatcher] Failed to send packet to worker {}, channel closed.",
                                worker_index
                            );
                            break;
                        }
                    } else {
                        debug!("[Dispatcher] Inbound channel closed, exiting.");
                        break;
                    }
                }
            }
        }
        debug!("[Dispatcher] stopped.");
    }

    fn create_interface(config: &NetStackConfig, device: &mut NetstackDevice) -> Interface {
        let mut iface_config = smoltcp::iface::Config::new(smoltcp::wire::HardwareAddress::Ip);
        iface_config.random_seed = rand::random();
        let mut iface =
            smoltcp::iface::Interface::new(iface_config, device, smoltcp::time::Instant::now());

        iface.set_any_ip(true);
        iface.update_ip_addrs(|ip_addrs| {
            let _ = ip_addrs.push(IpCidr::new(config.ipv4_addr.into(), config.ipv4_prefix_len));
            let _ = ip_addrs.push(IpCidr::new(config.ipv6_addr.into(), config.ipv6_prefix_len));
        });

        iface
            .routes_mut()
            .add_default_ipv4_route(config.ipv4_addr)
            .expect("Failed to add default IPv4 route");
        iface
            .routes_mut()
            .add_default_ipv6_route(config.ipv6_addr)
            .expect("Failed to add default IPv6 route");

        iface
    }
}

impl TcpConnectionWorker {
    async fn accept_loop(&mut self, mut device: NetstackDevice) -> std::io::Result<()> {
        loop {
            // --- STAGE 1: WORK-DRAINING ---
            let mut progress = true;
            while progress {
                progress = false;

                // Drain inbound packets
                while let Ok(packet) = self.inbound.try_recv() {
                    if let Err(_e) = self.process_inbound_frame(packet).await {
                        error!("Error processing inbound frame: {}", _e);
                    }
                    progress = true;
                }

                // Poll smoltcp for network events
                let now = smoltcp::time::Instant::now();
                if self.iface.poll(now, &mut device, &mut self.sockets) != PollResult::None {
                    progress = true;
                }

                // Handle IO for all active sockets
                let mut total_bytes_processed = 0;
                for (socket_handle, socket_control) in self.socket_maps.iter_mut() {
                    let socket = self.sockets.get_mut::<Socket>(*socket_handle);
                    let (read, written) = Self::handle_socket_io(socket, socket_control);
                    if read > 0 || written > 0 {
                        total_bytes_processed += read + written;
                    }
                }
                if total_bytes_processed > 0 {
                    progress = true;
                }

                // Prune any closed/aborted sockets
                if Self::prune_sockets(&mut self.sockets, &mut self.socket_maps) {
                    progress = true;
                }

                // Poll smoltcp again after IO
                let now = smoltcp::time::Instant::now();
                if self.iface.poll(now, &mut device, &mut self.sockets) != PollResult::None {
                    progress = true;
                }

                if progress && total_bytes_processed == 0 && self.inbound.is_empty() {
                    tokio::task::yield_now().await;
                }
            }

            // --- STAGE 2: IDLE / WAITING ---
            // If we are here, it means the work-draining loop completed a full iteration
            // without any work being done. We are now idle and can safely wait for the next event.

            let now = smoltcp::time::Instant::now();
            let smoltcp_delay = self.iface.poll_delay(now, &self.sockets).map(|d| d.into());

            tokio::select! {
                biased;
                _ = self.shutdown_rx.recv() => {
                    debug!("Worker received shutdown signal, exiting gracefully.");
                    return Ok(());
                }

                // Wait for a new inbound packet to arrive.
                maybe_packet = self.inbound.recv() => {
                    match maybe_packet {
                        Some(packet) => {
                            if let Err(_e) = self.process_inbound_frame(packet).await {
                                error!("Error processing inbound frame: {}", _e);
                            }
                            // After processing, continue to the top of 'main_loop to enter the work-draining phase.
                        },
                        None => return Ok(()), // Channel closed, exit worker.
                    }
                },

                // Wait for a notification from a TcpStream (e.g., app wrote data).
                _ = self.notifier.notified() => {
                    // Notification received. No action needed here.
                    // We'll simply loop back to the top and enter the work-draining phase.
                },

                // Wait for smoltcp's timer to expire (e.g., for retransmissions).
                _ = async {
                    match smoltcp_delay {
                        // If there's a specific delay, we sleep for that duration.
                        Some(delay) if delay > Duration::ZERO => tokio::time::sleep(delay).await,
                        // In all other cases (no timer, or timer is for "now"),
                        // we wait on a future that never completes. This effectively
                        // disables this select arm and forces the select to wait for
                        // one of the other arms (shutdown, inbound, notifier).
                        _ => std::future::pending().await,
                    }
                } => {
                    // Timer expired. No action needed here.
                    // We'll simply loop back to the top and the work-draining phase will poll smoltcp.
                },
            }
        }
    }

    async fn process_inbound_frame(&mut self, frame: Packet) -> std::io::Result<()> {
        if let Ok(ip_packet) = IpPacket::new_checked(frame.data())
            && ip_packet.protocol() == IpProtocol::Tcp
            && let Ok(tcp_packet) = TcpPacket::new_checked(ip_packet.payload())
            && tcp_packet.syn()
            && !tcp_packet.ack()
        {
            self.accept_new_connection(&ip_packet, &tcp_packet)?;
        }

        self.device_injector
            .try_send(frame)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        Ok(())
    }

    fn accept_new_connection(
        &mut self,
        ip_packet: &IpPacket<&[u8]>,
        tcp_packet: &TcpPacket<&[u8]>,
    ) -> std::io::Result<()> {
        let src_addr = SocketAddr::new(ip_packet.src_addr(), tcp_packet.src_port());
        let dst_addr = SocketAddr::new(ip_packet.dst_addr(), tcp_packet.dst_port());

        let mut socket = Socket::new(
            SocketBuffer::new(vec![0u8; self.config.tcp_recv_buffer_size]),
            SocketBuffer::new(vec![0u8; self.config.tcp_send_buffer_size]),
        );

        socket.set_keep_alive(Some(self.config.tcp_keep_alive.into()));
        socket.set_timeout(Some(self.config.tcp_timeout.into()));
        socket.set_nagle_enabled(false);
        socket.set_congestion_control(CongestionControl::Cubic);

        socket
            .listen(dst_addr)
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        let recv_rb = Arc::new(HeapRb::<u8>::new(self.config.tcp_recv_buffer_size));
        let (recv_prod, recv_cons) = (
            ringbuf::Prod::new(recv_rb.clone()),
            ringbuf::Cons::new(recv_rb),
        );

        let send_rb = Arc::new(HeapRb::<u8>::new(self.config.tcp_send_buffer_size));
        let (send_prod, send_cons) = (
            ringbuf::Prod::new(send_rb.clone()),
            ringbuf::Cons::new(send_rb),
        );

        let shared_state = Arc::new(SharedState::new());
        let stream = TcpStream {
            local_addr: src_addr,
            remote_addr: dst_addr,
            recv_buffer_cons: recv_cons,
            send_buffer_prod: send_prod,
            shared_state: shared_state.clone(),
            worker_notifier: self.notifier.clone(),
        };

        let io_handle = SocketIOHandle {
            recv_buffer_prod: recv_prod,
            send_buffer_cons: send_cons,
            shared_state,
        };

        if self.socket_stream_emitter.try_send(stream).is_ok() {
            let socket_handle = self.sockets.add(socket);
            self.socket_maps.insert(socket_handle, io_handle);
        } else {
            error!(
                "[Worker] Failed to emit new TcpStream to application, channel is full or closed. Dropping new connection from {}.",
                src_addr
            );
        }

        Ok(())
    }

    fn handle_socket_io(
        socket: &mut Socket,
        socket_control: &mut SocketIOHandle,
    ) -> (usize, usize) {
        let mut bytes_read = 0;
        let mut bytes_written = 0;
        let mut notify_read = false;

        if socket.can_recv() {
            match socket.recv(|buffer| {
                let n = socket_control.recv_buffer_prod.push_slice(buffer);
                if n > 0 {
                    bytes_read += n;
                }
                (n, buffer.len())
            }) {
                Ok(n) => {
                    if n > 0 {
                        notify_read = true;
                    }
                }
                Err(_e) => {
                    error!("Socket recv error: {}. Closing read side.", _e);
                    socket_control
                        .shared_state
                        .read_closed
                        .store(true, Ordering::Release);
                    notify_read = true;
                }
            }
        }

        if !socket.is_open()
            && !socket_control
                .shared_state
                .read_closed
                .load(Ordering::Acquire)
        {
            socket_control
                .shared_state
                .read_closed
                .store(true, Ordering::Release);
            notify_read = true;
        }

        if notify_read {
            socket_control.shared_state.recv_waker.wake();
        }

        let mut notify_write = false;

        while socket.can_send() && !socket_control.send_buffer_cons.is_empty() {
            match socket.send(|buffer| {
                let n = socket_control.send_buffer_cons.pop_slice(buffer);
                (n, buffer.len())
            }) {
                Ok(n) if n > 0 => {
                    bytes_written += n;
                    notify_write = true;
                }
                _ => break,
            }
        }

        if notify_write {
            socket_control.shared_state.send_waker.wake();
        }

        (bytes_read, bytes_written)
    }

    fn prune_sockets(
        sockets: &mut SocketSet,
        socket_maps: &mut HashMap<smoltcp::iface::SocketHandle, SocketIOHandle>,
    ) -> bool {
        let initial_len = socket_maps.len();
        socket_maps.retain(|handle, socket_control| {
            let socket = sockets.get_mut::<Socket>(*handle);

            if socket_control
                .shared_state
                .socket_dropped
                .load(Ordering::Acquire)
            {
                socket.abort();
            }

            if !socket.is_active() && socket.state() == State::Closed {
                sockets.remove(*handle);
                return false;
            }

            true
        });
        initial_len != socket_maps.len()
    }
}

impl futures::Stream for TcpConnection {
    type Item = TcpStream;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.socket_stream.poll_recv(cx)
    }
}

mod stream {
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll};

    use futures::task::AtomicWaker;
    use ringbuf::HeapRb;
    use ringbuf::traits::{Consumer, Observer, Producer};
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf, ReadHalf, WriteHalf};
    use tokio::sync::Notify;

    pub(crate) type RbProducer = ringbuf::Prod<Arc<HeapRb<u8>>>;
    pub(crate) type RbConsumer = ringbuf::Cons<Arc<HeapRb<u8>>>;

    pub(crate) struct SharedState {
        pub(crate) recv_waker: AtomicWaker,
        pub(crate) send_waker: AtomicWaker,
        pub(crate) read_closed: AtomicBool,
        pub(crate) socket_dropped: AtomicBool,
    }

    impl SharedState {
        pub fn new() -> Self {
            Self {
                recv_waker: AtomicWaker::new(),
                send_waker: AtomicWaker::new(),
                read_closed: AtomicBool::new(false),
                socket_dropped: AtomicBool::new(false),
            }
        }
    }

    pub struct TcpStream {
        pub(crate) local_addr: SocketAddr,
        pub(crate) remote_addr: SocketAddr,
        pub(crate) recv_buffer_cons: RbConsumer,
        pub(crate) send_buffer_prod: RbProducer,
        pub(crate) shared_state: Arc<SharedState>,
        pub(crate) worker_notifier: Arc<Notify>,
    }

    impl TcpStream {
        pub fn local_addr(&self) -> SocketAddr {
            self.local_addr
        }

        pub fn remote_addr(&self) -> SocketAddr {
            self.remote_addr
        }

        pub fn split(self) -> (ReadHalf<Self>, WriteHalf<Self>) {
            tokio::io::split(self)
        }
    }

    impl AsyncRead for TcpStream {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            if self.recv_buffer_cons.is_empty() {
                if self.shared_state.read_closed.load(Ordering::Acquire) {
                    return Poll::Ready(Ok(()));
                }
                self.shared_state.recv_waker.register(cx.waker());
                return Poll::Pending;
            }

            let unfilled_slice = buf.initialize_unfilled();
            let n = self.recv_buffer_cons.pop_slice(unfilled_slice);
            buf.advance(n);

            self.worker_notifier.notify_one();

            Poll::Ready(Ok(()))
        }
    }

    impl AsyncWrite for TcpStream {
        fn poll_write(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            if self.shared_state.socket_dropped.load(Ordering::Relaxed) {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "Socket is closing",
                )));
            }

            if self.send_buffer_prod.is_full() {
                self.shared_state.send_waker.register(cx.waker());
                return Poll::Pending;
            }

            let n = self.send_buffer_prod.push_slice(buf);
            if n > 0 {
                self.worker_notifier.notify_one();
            }

            Poll::Ready(Ok(n))
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            if !self.send_buffer_prod.is_empty() {
                self.shared_state.send_waker.register(cx.waker());
                self.worker_notifier.notify_one();
                return Poll::Pending;
            }
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            std::task::ready!(self.as_mut().poll_flush(cx))?;

            self.shared_state
                .socket_dropped
                .store(true, Ordering::Release);
            self.worker_notifier.notify_one();
            Poll::Ready(Ok(()))
        }
    }

    impl Drop for TcpStream {
        fn drop(&mut self) {
            self.shared_state
                .socket_dropped
                .store(true, Ordering::Release);
            self.worker_notifier.notify_one();
        }
    }
}
