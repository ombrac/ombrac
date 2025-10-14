use std::io;
use std::sync::Arc;
use std::time::Duration;

use bytes::{BufMut, Bytes, BytesMut};
use moka::future::Cache;
use tokio::sync::Mutex;

use crate::protocol::{Address, UdpPacket};

const DEFAULT_REASSEMBLY_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_MAX_CONCURRENT_REASSEMBLIES: u64 = 8192;

// The key for the cache is a combination of the session and fragment IDs
// to ensure fragments from different sessions don't collide.
type CacheKey = (u64, u16);
type SessionID = u64;

struct ReassemblyBuffer {
    fragments: Vec<Option<Bytes>>,
    received_count: u16,
    total_count: u16,
    address: Option<Address>,
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

    fn add_fragment(&mut self, fragment: UdpPacket) -> bool {
        if let UdpPacket::Fragmented {
            fragment_index,
            fragment_count,
            address,
            data,
            ..
        } = fragment
        {
            // If this is the first fragment we see, initialize the buffer properly
            if self.total_count == 0 {
                self.total_count = fragment_count;
                self.fragments = vec![None; fragment_count as usize];
            } else if self.total_count != fragment_count {
                // Mismatch in fragment count, likely a corrupted or malicious packet
                return false;
            }

            if self.address.is_none() && fragment_index == 0 {
                self.address = address;
            }

            let index = fragment_index as usize;
            if index < self.fragments.len() && self.fragments[index].is_none() {
                self.fragments[index] = Some(data);
                self.received_count += 1;
                return true;
            }
        }
        false
    }

    fn is_complete(&self) -> bool {
        self.total_count > 0 && self.received_count == self.total_count && self.address.is_some()
    }

    fn assemble_and_take(&mut self) -> Option<(SessionID, Address, Bytes)> {
        if !self.is_complete() {
            return None;
        }

        let mut combined = BytesMut::new();
        for fragment in self.fragments.iter_mut() {
            if let Some(data) = fragment.take() {
                combined.put(data);
            } else {
                // Should not happen if is_complete is true
                return None;
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

    pub async fn process(&self, packet: UdpPacket) -> io::Result<Option<(u64, Address, Bytes)>> {
        match packet {
            UdpPacket::Unfragmented {
                session_id,
                address,
                data,
            } => Ok(Some((session_id, address, data))),
            UdpPacket::Fragmented {
                session_id,
                fragment_id,
                ..
            } => {
                let cache_key = (session_id, fragment_id);

                // Get or insert a new buffer. This is atomic.
                let buffer_lock = self
                    .cache
                    .get_with(cache_key, async {
                        // For now, create a buffer with unknown total_count and address.
                        // These will be filled in when the relevant fragment arrives.
                        Arc::new(Mutex::new(ReassemblyBuffer::new(session_id, 0, None)))
                    })
                    .await;

                let mut buffer = buffer_lock.lock().await;

                // Add the fragment. If it's a duplicate, this will do nothing.
                buffer.add_fragment(packet);

                if buffer.is_complete() {
                    let assembled_data = buffer.assemble_and_take();
                    self.cache.invalidate(&cache_key).await;
                    return Ok(assembled_data);
                }

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
        fragment_id: u16,
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
