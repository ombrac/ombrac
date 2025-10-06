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
    received_count: u8,
    total_count: u8,
    address: Address,
    session_id: SessionID,
}

impl ReassemblyBuffer {
    fn new(first_fragment: UdpPacket) -> Option<Self> {
        if let UdpPacket::Fragmented {
            session_id,
            fragment_index: 0,
            fragment_count,
            address: Some(address),
            data,
            ..
        } = first_fragment
        {
            let mut fragments = vec![None; fragment_count as usize];
            fragments[0] = Some(data);
            Some(Self {
                fragments,
                received_count: 1,
                total_count: fragment_count,
                address,
                session_id,
            })
        } else {
            None
        }
    }

    fn add_fragment(&mut self, fragment: UdpPacket) {
        if let UdpPacket::Fragmented {
            fragment_index,
            data,
            ..
        } = fragment
        {
            let index = fragment_index as usize;
            if index < self.fragments.len() && self.fragments[index].is_none() {
                self.fragments[index] = Some(data);
                self.received_count += 1;
            }
        }
    }

    fn is_complete(&self) -> bool {
        self.received_count > 0 && self.received_count == self.total_count
    }

    fn assemble_and_take(&mut self) -> (SessionID, Address, Bytes) {
        let mut combined = BytesMut::new();

        for fragment in self.fragments.iter_mut() {
            if let Some(data) = fragment.take() {
                combined.put(data);
            }
        }
        (self.session_id, self.address.clone(), combined.freeze())
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
                fragment_index,
                ..
            } => {
                let cache_key = (session_id, fragment_id);
                if fragment_index == 0 {
                    if let Some(mut buffer) = ReassemblyBuffer::new(packet) {
                        if buffer.is_complete() {
                            return Ok(Some(buffer.assemble_and_take()));
                        }
                        let buffer_lock = Arc::new(Mutex::new(buffer));
                        self.cache.insert(cache_key, buffer_lock).await;
                    } else {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "First fragment packet is malformed",
                        ));
                    }
                } else if let Some(buffer_lock) = self.cache.get(&cache_key).await {
                    let mut buffer = buffer_lock.lock().await;
                    buffer.add_fragment(packet);

                    if buffer.is_complete() {
                        let assembled_data = buffer.assemble_and_take();
                        self.cache.invalidate(&cache_key).await;
                        return Ok(Some(assembled_data));
                    }
                }
                Ok(None)
            }
        }
    }
}
