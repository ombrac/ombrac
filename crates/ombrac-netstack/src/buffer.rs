use std::io;
use std::mem;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use bytes::{BufMut, BytesMut};
use crossbeam_queue::SegQueue;

#[derive(Clone)]
pub struct BufferPool {
    pool: Arc<SegQueue<BytesMut>>,
    max_pool_size: usize,
    default_buffer_size: usize,
}

impl BufferPool {
    pub fn new(max_pool_size: usize, default_buffer_size: usize) -> Self {
        Self {
            pool: Arc::new(SegQueue::new()),
            max_pool_size,
            default_buffer_size,
        }
    }

    pub fn get(&self, capacity: usize) -> PooledBytesMut {
        let required_capacity = std::cmp::max(capacity, self.default_buffer_size);
        let mut buffer = self
            .pool
            .pop()
            .unwrap_or_else(|| BytesMut::with_capacity(required_capacity));

        if buffer.capacity() < required_capacity {
            buffer.reserve(required_capacity - buffer.capacity());
        }

        buffer.clear();

        PooledBytesMut {
            buffer,
            pool: self.clone(),
        }
    }

    fn release(&self, buffer: BytesMut) {
        if self.pool.len() < self.max_pool_size {
            self.pool.push(buffer);
        }
    }
}

pub struct PooledBytesMut {
    buffer: BytesMut,
    pool: BufferPool,
}

impl Drop for PooledBytesMut {
    fn drop(&mut self) {
        let buffer = mem::take(&mut self.buffer);
        self.pool.release(buffer);
    }
}

impl Deref for PooledBytesMut {
    type Target = BytesMut;
    fn deref(&self) -> &Self::Target {
        &self.buffer
    }
}

impl DerefMut for PooledBytesMut {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.buffer
    }
}

impl io::Write for PooledBytesMut {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.put_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::thread;

    #[test]
    fn get_returns_buffer_with_at_least_capacity() {
        let pool = BufferPool::new(8, 1024);
        let buf = pool.get(2048);
        assert!(buf.capacity() >= 2048);
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn get_uses_default_when_capacity_smaller() {
        let pool = BufferPool::new(8, 1024);
        let buf = pool.get(64);
        assert!(buf.capacity() >= 1024);
    }

    #[test]
    fn buffer_returns_to_pool_on_drop() {
        let pool = BufferPool::new(4, 256);
        assert_eq!(pool.pool.len(), 0);
        let buf = pool.get(256);
        assert_eq!(pool.pool.len(), 0);
        drop(buf);
        assert_eq!(pool.pool.len(), 1);
    }

    #[test]
    fn pool_respects_max_pool_size() {
        let pool = BufferPool::new(2, 64);

        let mut bufs = Vec::new();
        for _ in 0..5 {
            bufs.push(pool.get(64));
        }
        drop(bufs);

        // Cannot exceed max_pool_size.
        assert!(pool.pool.len() <= 2);
    }

    #[test]
    fn pooled_buffer_is_cleared_on_get() {
        let pool = BufferPool::new(4, 64);
        {
            let mut buf = pool.get(64);
            buf.put_slice(b"hello");
            assert_eq!(buf.len(), 5);
        }
        let buf = pool.get(64);
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn write_trait_appends_data() {
        let pool = BufferPool::new(4, 64);
        let mut buf = pool.get(64);
        buf.write_all(b"abc").unwrap();
        buf.write_all(b"def").unwrap();
        assert_eq!(&buf[..], b"abcdef");
    }

    #[test]
    fn pool_is_thread_safe() {
        let pool = BufferPool::new(16, 256);
        let mut handles = Vec::new();
        for _ in 0..8 {
            let pool = pool.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    let mut b = pool.get(256);
                    b.put_slice(&[0xab; 32]);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        // Pool size should not exceed max
        assert!(pool.pool.len() <= 16);
    }

    #[test]
    fn growing_capacity_works() {
        let pool = BufferPool::new(4, 64);
        let mut buf = pool.get(64);
        buf.put_slice(&[1u8; 64]);
        // Reuse asks for larger size – buffer should expand.
        drop(buf);
        let buf = pool.get(2048);
        assert!(buf.capacity() >= 2048);
    }
}
