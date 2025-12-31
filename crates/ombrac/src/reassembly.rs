use std::io;
use std::sync::Arc;
use std::time::Duration;

use bytes::{BufMut, Bytes, BytesMut};
use moka::future::Cache;
use tokio::sync::Mutex;

use crate::protocol::{Address, UdpPacket};

/// Default timeout for fragment reassembly [10 seconds]
const DEFAULT_REASSEMBLY_TIMEOUT: Duration = Duration::from_secs(10);

/// Default maximum number of concurrent reassembly operations [8192]
const DEFAULT_MAX_CONCURRENT_REASSEMBLIES: u64 = 8192;

/// Cache key type: (session_id, fragment_id)
///
/// The combination ensures fragments from different sessions don't collide,
/// and different fragmentation operations within the same session are handled separately.
type CacheKey = (u64, u32);

/// Session identifier type
type SessionID = u64;

/// Buffer for reassembling fragmented UDP packets.
///
/// Tracks received fragments and assembles them into a complete packet
/// when all fragments have been received.
struct ReassemblyBuffer {
    /// Fragment storage (None = not received yet)
    fragments: Vec<Option<Bytes>>,
    /// Number of fragments received so far
    received_count: u16,
    /// Total number of fragments expected
    total_count: u16,
    /// Destination address (only in first fragment)
    address: Option<Address>,
    /// Session identifier
    session_id: SessionID,
}

impl ReassemblyBuffer {
    fn new(session_id: SessionID, fragment_count: u16, address: Option<Address>) -> Self {
        Self {
            fragments: vec![None; fragment_count as usize],
            received_count: 0,
            total_count: fragment_count,
            address,
            session_id,
        }
    }

    /// Adds a fragment to the buffer.
    ///
    /// # Returns
    ///
    /// `true` if the fragment was successfully added, `false` if it was invalid
    /// or a duplicate.
    fn add_fragment(&mut self, fragment: UdpPacket) -> bool {
        let UdpPacket::Fragmented {
            fragment_index,
            fragment_count,
            address,
            data,
            ..
        } = fragment
        else {
            return false;
        };

        // Validate fragment_count
        if fragment_count == 0 {
            return false; // Invalid fragment
        }

        // Initialize the buffer with the fragment_count from the first fragment we see.
        if self.total_count == 0 {
            self.total_count = fragment_count;
            self.fragments = vec![None; fragment_count as usize];
        } else if self.total_count != fragment_count {
            // Mismatch in fragment count, likely a corrupted or malicious packet.
            return false;
        }

        // The address is only present in the first fragment.
        if fragment_index == 0 {
            match &self.address {
                None => {
                    // First time seeing address, store it
                    self.address = address;
                }
                Some(existing) => {
                    // Check if the address matches
                    if let Some(new_addr) = &address {
                        if existing != new_addr {
                            // Address mismatch, could be an attack or a hash collision.
                            return false;
                        }
                    } else {
                        // First fragment should always have an address
                        return false;
                    }
                }
            }
        }

        // Validate fragment_index and check if we already have this fragment
        let index = fragment_index as usize;
        if index < self.fragments.len() && self.fragments[index].is_none() {
            self.fragments[index] = Some(data);
            self.received_count += 1;
            return true;
        }

        false
    }

    /// Checks if all fragments have been received and the buffer is complete.
    fn is_complete(&self) -> bool {
        self.total_count > 0 && self.received_count == self.total_count && self.address.is_some()
    }

    /// Assembles the fragments into a complete packet and takes ownership.
    ///
    /// # Returns
    ///
    /// `Some((session_id, address, data))` if the buffer is complete, `None` otherwise.
    fn assemble_and_take(&mut self) -> Option<(SessionID, Address, Bytes)> {
        if !self.is_complete() {
            return None;
        }

        // Pre-allocate capacity for better performance
        let mut combined = BytesMut::with_capacity(
            self.fragments
                .iter()
                .map(|f| f.as_ref().map(|b| b.len()).unwrap_or(0))
                .sum(),
        );

        // Assemble fragments in order
        for fragment in self.fragments.iter_mut() {
            match fragment.take() {
                Some(data) => combined.put(data),
                None => {
                    // Should not happen if is_complete is true
                    return None;
                }
            }
        }

        self.address
            .clone()
            .map(|addr| (self.session_id, addr, combined.freeze()))
    }
}

pub struct UdpReassembler {
    cache: Cache<CacheKey, Arc<Mutex<ReassemblyBuffer>>>,
}

impl Default for UdpReassembler {
    fn default() -> Self {
        Self::new(
            DEFAULT_MAX_CONCURRENT_REASSEMBLIES,
            DEFAULT_REASSEMBLY_TIMEOUT,
        )
    }
}

impl UdpReassembler {
    pub fn new(max_concurrent_reassemblies: u64, reassembly_timeout: Duration) -> Self {
        let cache = Cache::builder()
            .max_capacity(max_concurrent_reassemblies)
            .time_to_live(reassembly_timeout)
            .build();

        Self { cache }
    }

    /// Processes a UDP packet, handling reassembly if fragmented.
    ///
    /// # Returns
    ///
    /// - `Ok(Some((session_id, address, data)))` - Complete packet ready
    /// - `Ok(None)` - Fragment received, waiting for more
    /// - `Err(e)` - Reassembly error
    pub async fn process(&self, packet: UdpPacket) -> io::Result<Option<(u64, Address, Bytes)>> {
        match packet {
            UdpPacket::Unfragmented {
                session_id,
                address,
                data,
            } => {
                // Unfragmented packet, return immediately
                Ok(Some((session_id, address, data)))
            }
            UdpPacket::Fragmented {
                session_id,
                fragment_id,
                ..
            } => {
                let cache_key = (session_id, fragment_id);

                // Get or insert a new buffer. This is atomic and prevents race conditions.
                let buffer_lock = self
                    .cache
                    .get_with(cache_key, async {
                        // Create a buffer with unknown total_count and address.
                        // These will be filled in when the relevant fragment arrives.
                        Arc::new(Mutex::new(ReassemblyBuffer::new(session_id, 0, None)))
                    })
                    .await;

                let mut buffer = buffer_lock.lock().await;

                // Add the fragment. If it's a duplicate or invalid, this returns false.
                if !buffer.add_fragment(packet) {
                    // Invalid or duplicate fragment, ignore it
                    return Ok(None);
                }

                // Check if we have all fragments
                if buffer.is_complete() {
                    let assembled_data = buffer.assemble_and_take();
                    // Remove from cache to free memory
                    self.cache.invalidate(&cache_key).await;
                    return Ok(assembled_data);
                }

                // Still waiting for more fragments
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Address;
    use bytes::Bytes;
    use std::net::{Ipv4Addr, SocketAddrV4};

    fn create_test_fragments(
        session_id: u64,
        fragment_id: u32,
        data: &Bytes,
        chunk_size: usize,
    ) -> Vec<UdpPacket> {
        let address = Address::SocketV4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 8080));
        let data_chunks: Vec<Bytes> = data
            .chunks(chunk_size)
            .map(Bytes::copy_from_slice)
            .collect();
        let fragment_count = data_chunks.len() as u16;

        data_chunks
            .into_iter()
            .enumerate()
            .map(move |(i, chunk)| {
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
            .collect()
    }

    #[tokio::test]
    async fn test_reassembly_in_order() {
        let reassembler = UdpReassembler::default();
        let original_data = Bytes::from(vec![0; 1024]);
        let fragments = create_test_fragments(1, 1, &original_data, 256);
        assert_eq!(fragments.len(), 4);

        let mut result = None;
        for fragment in fragments {
            result = reassembler.process(fragment).await.unwrap();
        }

        assert!(result.is_some());
        let (session_id, _, data) = result.unwrap();
        assert_eq!(session_id, 1);
        assert_eq!(data, original_data);
    }

    #[tokio::test]
    async fn test_reassembly_out_of_order() {
        let reassembler = UdpReassembler::default();
        let original_data = Bytes::from(vec![1; 1024]);
        let mut fragments = create_test_fragments(2, 2, &original_data, 256);
        // Reorder fragments: 2, 0, 3, 1
        fragments.swap(0, 2);
        fragments.swap(1, 3);

        let mut result = None;
        for fragment in fragments {
            result = reassembler.process(fragment).await.unwrap();
        }

        assert!(result.is_some());
        let (session_id, _, data) = result.unwrap();
        assert_eq!(session_id, 2);
        assert_eq!(data, original_data);
    }

    #[tokio::test]
    async fn test_reassembly_with_duplicates() {
        let reassembler = UdpReassembler::default();
        let original_data = Bytes::from(vec![2; 512]);
        let mut fragments = create_test_fragments(3, 3, &original_data, 256);
        // Duplicate fragment 0 and 1
        fragments.push(fragments[0].clone());
        fragments.push(fragments[1].clone());

        let mut result = None;
        for fragment in fragments {
            let res = reassembler.process(fragment).await.unwrap();
            if res.is_some() {
                result = res;
            }
        }

        assert!(result.is_some());
        let (session_id, _, data) = result.unwrap();
        assert_eq!(session_id, 3);
        assert_eq!(data, original_data);
    }

    #[tokio::test]
    async fn test_incomplete_reassembly() {
        let reassembler = UdpReassembler::default();
        let original_data = Bytes::from(vec![3; 1024]);
        let mut fragments = create_test_fragments(4, 4, &original_data, 256);
        // Remove one fragment
        fragments.pop();

        let mut result = None;
        for fragment in fragments {
            result = reassembler.process(fragment).await.unwrap();
        }

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_malformed_first_fragment_no_address() {
        let reassembler = UdpReassembler::default();
        let original_data = Bytes::from(vec![4; 512]);
        let mut fragments = create_test_fragments(5, 5, &original_data, 256);

        // Tamper with the first fragment to remove the address
        if let UdpPacket::Fragmented { address, .. } = &mut fragments[0] {
            *address = None;
        }

        let mut result = None;
        for fragment in fragments {
            result = reassembler.process(fragment).await.unwrap();
        }

        // Should not complete because the address is missing
        assert!(result.is_none());
    }
}
