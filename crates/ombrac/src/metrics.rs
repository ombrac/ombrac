//! Lightweight runtime metrics.
//!
//! `Metrics` is an `Arc`-clonable bag of atomic counters intended to be embedded
//! in long-lived components (`ConnectionAcceptor` on the server, `Client` on the
//! client side). Increments are `Ordering::Relaxed` — cheap on the hot path,
//! safe to read concurrently. Use `snapshot()` for an atomic-ish point-in-time
//! view to feed into Prometheus / logging / health endpoints.
//!
//! The default is a zeroed instance; consumers that don't care can ignore it
//! and pay only the cost of a few relaxed atomic increments (negligible vs
//! the QUIC and crypto cost of any real operation).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Counters tracked across the lifetime of a server or client.
#[derive(Debug, Default)]
pub struct Counters {
    /// Successful incoming connection acceptances (post-handshake).
    pub connections_accepted: AtomicU64,
    /// Incoming connections rejected (e.g. exceeded max-connections limit).
    pub connections_rejected: AtomicU64,
    /// Auth/handshake failures.
    pub connections_auth_failed: AtomicU64,

    /// Bidirectional streams successfully opened on a tunnel.
    pub streams_opened: AtomicU64,
    /// Bidirectional streams closed (any reason).
    pub streams_closed: AtomicU64,
    /// Stream open requests that failed (destination refused, DNS, etc).
    pub streams_failed: AtomicU64,

    /// UDP sessions opened.
    pub udp_sessions_opened: AtomicU64,
    /// UDP sessions closed.
    pub udp_sessions_closed: AtomicU64,

    /// Total bytes received from the tunnel peer.
    pub bytes_rx: AtomicU64,
    /// Total bytes sent to the tunnel peer.
    pub bytes_tx: AtomicU64,

    /// UDP packet reassemblies completed successfully.
    pub reassemblies_completed: AtomicU64,
    /// UDP packet fragments dropped (invalid / duplicate / timeout).
    pub reassembly_drops: AtomicU64,

    /// Client-side reconnect attempts (including failed retries).
    pub reconnect_attempts: AtomicU64,
    /// Client-side reconnects that successfully re-authenticated.
    pub reconnect_succeeded: AtomicU64,
}

/// Cheap-to-clone handle to a shared set of counters.
///
/// Internally `Arc`, so cloning is just a refcount bump.
#[derive(Debug, Clone, Default)]
pub struct Metrics(Arc<Counters>);

impl Metrics {
    /// Returns a fresh `Metrics` with all counters at zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Borrows the inner counters for direct atomic increments.
    ///
    /// Callers on the hot path can do `metrics.counters().streams_opened.fetch_add(1, Ordering::Relaxed)`.
    pub fn counters(&self) -> &Counters {
        &self.0
    }

    /// Reads all counter values into a plain struct, suitable for exporting.
    ///
    /// Each load is `Relaxed` — the snapshot is "eventually consistent" across
    /// fields, not a global atomic snapshot. That tradeoff is acceptable for
    /// telemetry but means callers should not derive invariants like
    /// `streams_opened - streams_closed = currently_open` exactly.
    pub fn snapshot(&self) -> MetricsSnapshot {
        let c = &self.0;
        MetricsSnapshot {
            connections_accepted: c.connections_accepted.load(Ordering::Relaxed),
            connections_rejected: c.connections_rejected.load(Ordering::Relaxed),
            connections_auth_failed: c.connections_auth_failed.load(Ordering::Relaxed),
            streams_opened: c.streams_opened.load(Ordering::Relaxed),
            streams_closed: c.streams_closed.load(Ordering::Relaxed),
            streams_failed: c.streams_failed.load(Ordering::Relaxed),
            udp_sessions_opened: c.udp_sessions_opened.load(Ordering::Relaxed),
            udp_sessions_closed: c.udp_sessions_closed.load(Ordering::Relaxed),
            bytes_rx: c.bytes_rx.load(Ordering::Relaxed),
            bytes_tx: c.bytes_tx.load(Ordering::Relaxed),
            reassemblies_completed: c.reassemblies_completed.load(Ordering::Relaxed),
            reassembly_drops: c.reassembly_drops.load(Ordering::Relaxed),
            reconnect_attempts: c.reconnect_attempts.load(Ordering::Relaxed),
            reconnect_succeeded: c.reconnect_succeeded.load(Ordering::Relaxed),
        }
    }
}

/// Plain, copyable snapshot of metric values at a single point in time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MetricsSnapshot {
    pub connections_accepted: u64,
    pub connections_rejected: u64,
    pub connections_auth_failed: u64,
    pub streams_opened: u64,
    pub streams_closed: u64,
    pub streams_failed: u64,
    pub udp_sessions_opened: u64,
    pub udp_sessions_closed: u64,
    pub bytes_rx: u64,
    pub bytes_tx: u64,
    pub reassemblies_completed: u64,
    pub reassembly_drops: u64,
    pub reconnect_attempts: u64,
    pub reconnect_succeeded: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_snapshot_is_all_zero() {
        let m = Metrics::new();
        let snap = m.snapshot();
        assert_eq!(snap, MetricsSnapshot::default());
    }

    #[test]
    fn counters_are_independent_per_instance() {
        let a = Metrics::new();
        let b = Metrics::new();
        a.counters().streams_opened.fetch_add(5, Ordering::Relaxed);
        assert_eq!(a.snapshot().streams_opened, 5);
        assert_eq!(b.snapshot().streams_opened, 0);
    }

    #[test]
    fn clone_shares_storage() {
        let a = Metrics::new();
        let b = a.clone();
        a.counters().streams_opened.fetch_add(3, Ordering::Relaxed);
        b.counters().streams_opened.fetch_add(4, Ordering::Relaxed);
        assert_eq!(a.snapshot().streams_opened, 7);
        assert_eq!(b.snapshot().streams_opened, 7);
    }

    #[test]
    fn snapshot_captures_all_fields() {
        let m = Metrics::new();
        let c = m.counters();
        c.connections_accepted.fetch_add(1, Ordering::Relaxed);
        c.connections_rejected.fetch_add(2, Ordering::Relaxed);
        c.connections_auth_failed.fetch_add(3, Ordering::Relaxed);
        c.streams_opened.fetch_add(4, Ordering::Relaxed);
        c.streams_closed.fetch_add(5, Ordering::Relaxed);
        c.streams_failed.fetch_add(6, Ordering::Relaxed);
        c.udp_sessions_opened.fetch_add(7, Ordering::Relaxed);
        c.udp_sessions_closed.fetch_add(8, Ordering::Relaxed);
        c.bytes_rx.fetch_add(9, Ordering::Relaxed);
        c.bytes_tx.fetch_add(10, Ordering::Relaxed);
        c.reassemblies_completed.fetch_add(11, Ordering::Relaxed);
        c.reassembly_drops.fetch_add(12, Ordering::Relaxed);
        c.reconnect_attempts.fetch_add(13, Ordering::Relaxed);
        c.reconnect_succeeded.fetch_add(14, Ordering::Relaxed);

        let s = m.snapshot();
        assert_eq!(s.connections_accepted, 1);
        assert_eq!(s.connections_rejected, 2);
        assert_eq!(s.connections_auth_failed, 3);
        assert_eq!(s.streams_opened, 4);
        assert_eq!(s.streams_closed, 5);
        assert_eq!(s.streams_failed, 6);
        assert_eq!(s.udp_sessions_opened, 7);
        assert_eq!(s.udp_sessions_closed, 8);
        assert_eq!(s.bytes_rx, 9);
        assert_eq!(s.bytes_tx, 10);
        assert_eq!(s.reassemblies_completed, 11);
        assert_eq!(s.reassembly_drops, 12);
        assert_eq!(s.reconnect_attempts, 13);
        assert_eq!(s.reconnect_succeeded, 14);
    }
}
