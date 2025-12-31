use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use bytes::Bytes;
use dashmap::DashMap;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use ombrac::protocol::{Address, UdpPacket};
use ombrac::reassembly::UdpReassembler;
use ombrac_macros::{debug, warn};
use ombrac_transport::{Connection, Initiator};

use super::ClientConnection;

// --- Datagram Configuration ---
/// Initial delay for datagram retry [default: 1 second]
const DATAGRAM_INITIAL_DELAY: Duration = Duration::from_secs(1);

/// Maximum delay for datagram retry [default: 60 seconds]
const DATAGRAM_MAX_DELAY: Duration = Duration::from_secs(60);

/// Channel buffer size for UDP session dispatcher [default: 128]
const UDP_SESSION_CHANNEL_BUFFER_SIZE: usize = 128;

/// Default tunnel MTU if transport doesn't provide one [default: 1350 bytes]
const DEFAULT_TUNNEL_MTU: usize = 1350;

type UdpSessionSender = mpsc::Sender<(Bytes, Address)>;

/// Manages all active UDP sessions and dispatches incoming datagrams.
pub struct UdpDispatcher {
    // Maps a session_id to a sender that forwards data to the `UdpSession`.
    dispatch_map: DashMap<u64, UdpSessionSender>,
    fragment_id_counter: AtomicU32,
}

impl UdpDispatcher {
    pub fn new() -> Self {
        Self {
            dispatch_map: DashMap::new(),
            fragment_id_counter: AtomicU32::new(0),
        }
    }

    /// The main loop for the background UDP dispatcher task.
    ///
    /// It continuously reads datagrams from the server, reassembles them,
    /// and forwards them to the correct `UdpSession`.
    pub async fn run<T, C>(
        connection: Arc<ClientConnection<T, C>>,
        dispatcher: Arc<Self>,
        shutdown_token: CancellationToken,
    ) where
        T: Initiator<Connection = C>,
        C: Connection,
    {
        let mut reassembler = UdpReassembler::default();
        let mut current_delay = DATAGRAM_INITIAL_DELAY;

        loop {
            tokio::select! {
                // Listen for the shutdown signal.
                _ = shutdown_token.cancelled() => {
                    break;
                }
                // Read the next datagram from the server.
                result = read_datagram(connection.as_ref(), &mut reassembler) => {
                    match result {
                        Ok((session_id, address, data)) => {
                            if current_delay != DATAGRAM_INITIAL_DELAY {
                                current_delay = DATAGRAM_INITIAL_DELAY;
                            }

                            dispatcher.dispatch(session_id, data, address).await;
                        }
                        Err(_e) => {
                            warn!("Error reading datagram: {}. Retrying in {:?}...", _e, current_delay);
                            tokio::time::sleep(current_delay).await;
                            current_delay = (current_delay * 2).min(DATAGRAM_MAX_DELAY);
                        }
                    }
                }
            }
        }
    }

    /// Forwards a received datagram to the appropriate session.
    pub async fn dispatch(&self, session_id: u64, data: Bytes, address: Address) {
        if let Some(tx) = self.dispatch_map.get(&session_id) {
            // If sending fails, the receiver (`UdpSession`) has been dropped.
            // It's safe to clean up the entry from the map.
            if tx.send((data, address)).await.is_err() {
                self.dispatch_map.remove(&session_id);
            }
        } else {
            warn!(
                "[Session][{}] Received datagram for UNKNOWN or CLOSED",
                session_id
            );
        }
    }

    /// Registers a new session and returns a receiver for it.
    pub fn register_session(&self, session_id: u64) -> mpsc::Receiver<(Bytes, Address)> {
        let (tx, rx) = mpsc::channel(UDP_SESSION_CHANNEL_BUFFER_SIZE);
        self.dispatch_map.insert(session_id, tx);
        rx
    }

    /// Unregisters a session when it is dropped.
    pub fn unregister_session(&self, session_id: u64) {
        self.dispatch_map.remove(&session_id);
    }

    /// Provides access to the fragment ID counter for sending datagrams.
    pub fn fragment_id_counter(&self) -> &AtomicU32 {
        &self.fragment_id_counter
    }
}

/// Reads a UDP datagram from the connection, handling reassembly.
async fn read_datagram<T, C>(
    connection: &ClientConnection<T, C>,
    reassembler: &mut UdpReassembler,
) -> io::Result<(u64, Address, Bytes)>
where
    T: Initiator<Connection = C>,
    C: Connection,
{
    loop {
        let packet_bytes = connection
            .with_retry(|conn| async move { conn.read_datagram().await })
            .await?;

        let packet = match UdpPacket::decode(&packet_bytes) {
            Ok(packet) => packet,
            Err(_e) => {
                warn!("Failed to decode UDP packet: {}. Discarding.", _e);
                continue; // Skip malformed packets.
            }
        };

        match reassembler.process(packet).await {
            Ok(Some((session_id, address, data))) => {
                return Ok((session_id, address, data));
            }
            Ok(None) => {
                continue; // Fragment received, continue reading.
            }
            Err(_e) => {
                warn!("Reassembly error: {}. Discarding fragment.", _e);
                continue; // Reassembly error, wait for the next valid packet.
            }
        }
    }
}

/// Sends a UDP datagram, handling fragmentation if necessary.
pub(crate) async fn send_datagram<T, C>(
    connection: &ClientConnection<T, C>,
    session_id: u64,
    dest_addr: Address,
    data: Bytes,
    fragment_id_counter: &AtomicU32,
) -> io::Result<()>
where
    T: Initiator<Connection = C>,
    C: Connection,
{
    if data.is_empty() {
        return Ok(());
    }

    let conn = connection.connection();
    // Use a conservative default MTU if the transport doesn't provide one.
    let max_datagram_size = conn.max_datagram_size().unwrap_or(DEFAULT_TUNNEL_MTU);
    // Leave a reasonable margin for headers.
    let overhead = UdpPacket::fragmented_overhead();
    let max_payload_size = max_datagram_size.saturating_sub(overhead).max(1);

    if data.len() <= max_payload_size {
        let packet = UdpPacket::Unfragmented {
            session_id,
            address: dest_addr.clone(),
            data,
        };
        let encoded = packet.encode()?;
        connection
            .with_retry(|conn| {
                let data_for_attempt = encoded.clone();
                async move { conn.send_datagram(data_for_attempt).await }
            })
            .await?;
    } else {
        // The packet is too large and must be fragmented.
        debug!(
            "[Session][{}] Sending packet for {} is too large ({} > max {}), fragmenting...",
            session_id,
            dest_addr,
            data.len(),
            max_payload_size
        );

        let fragment_id = fragment_id_counter.fetch_add(1, Ordering::Relaxed);
        let fragments =
            UdpPacket::split_packet(session_id, dest_addr, data, max_payload_size, fragment_id);

        for fragment in fragments {
            let packet_bytes = fragment.encode()?;
            connection
                .with_retry(|conn| {
                    let data_for_attempt = packet_bytes.clone();
                    async move { conn.send_datagram(data_for_attempt).await }
                })
                .await?;
        }
    }
    Ok(())
}

/// Represents a virtual UDP session over the tunnel.
pub struct UdpSession<T, C>
where
    T: Initiator<Connection = C>,
    C: Connection,
{
    session_id: u64,
    connection: Arc<ClientConnection<T, C>>,
    dispatcher: Arc<UdpDispatcher>,
    receiver: mpsc::Receiver<(Bytes, Address)>,
}

impl<T, C> UdpSession<T, C>
where
    T: Initiator<Connection = C>,
    C: Connection,
{
    /// Creates a new `UdpSession`.
    pub(crate) fn new(
        session_id: u64,
        connection: Arc<ClientConnection<T, C>>,
        dispatcher: Arc<UdpDispatcher>,
        receiver: mpsc::Receiver<(Bytes, Address)>,
    ) -> Self {
        Self {
            session_id,
            connection,
            dispatcher,
            receiver,
        }
    }

    /// Sends a UDP datagram to the specified destination through the tunnel.
    pub async fn send_to(&self, data: Bytes, dest_addr: Address) -> io::Result<()> {
        send_datagram(
            &self.connection,
            self.session_id,
            dest_addr,
            data,
            self.dispatcher.fragment_id_counter(),
        )
        .await
    }

    /// Receives a UDP datagram from the tunnel for this session.
    ///
    /// Returns the received data and its original sender address.
    pub async fn recv_from(&mut self) -> Option<(Bytes, Address)> {
        self.receiver.recv().await
    }
}

impl<T, C> Drop for UdpSession<T, C>
where
    T: Initiator<Connection = C>,
    C: Connection,
{
    fn drop(&mut self) {
        // When a session is dropped, remove its dispatcher from the map
        // to prevent the map from growing indefinitely.
        self.dispatcher.unregister_session(self.session_id);
    }
}
