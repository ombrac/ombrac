use bytes::Bytes;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use ombrac::protocol::{Address, UdpPacket};
use ombrac::reassembly::UdpReassembler;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;

// ── Concurrent reassembly sessions ───────────────────────────────────────────

fn bench_concurrent_sessions(c: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();

    let addr = Address::SocketV4(SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 1), 9000));
    let data_size = 8192usize;
    let chunk = 1200usize;
    let session_counts: &[usize] = &[1, 8, 64, 256];

    let mut group = c.benchmark_group("concurrency/reassembly_sessions");
    for &sessions in session_counts {
        let total_bytes = (data_size * sessions) as u64;
        group.throughput(Throughput::Bytes(total_bytes));

        group.bench_with_input(
            BenchmarkId::new("sessions", sessions),
            &sessions,
            |b, &n| {
                b.to_async(&runtime).iter(|| {
                    let addr = addr.clone();
                    async move {
                        let reassembler = Arc::new(UdpReassembler::default());
                        let mut handles = Vec::with_capacity(n);

                        for session_id in 0..n as u64 {
                            let r = reassembler.clone();
                            let a = addr.clone();
                            let data = Bytes::from(vec![(session_id % 256) as u8; data_size]);
                            let fragments: Vec<UdpPacket> =
                                UdpPacket::split_packet(session_id, a, data, chunk, 0).collect();

                            handles.push(tokio::spawn(async move {
                                let mut result = None;
                                for f in fragments {
                                    result = r.process(f).await.unwrap();
                                }
                                assert!(result.is_some());
                            }));
                        }

                        for h in handles {
                            h.await.unwrap();
                        }
                    }
                });
            },
        );
    }
    group.finish();
}

// ── Concurrent encode/decode ──────────────────────────────────────────────────

fn bench_concurrent_encode(c: &mut Criterion) {
    use ombrac::protocol::{ClientConnect, encode};

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();

    let msg = ClientConnect {
        address: Address::SocketV4(SocketAddrV4::new(Ipv4Addr::new(1, 2, 3, 4), 443)),
    };
    let worker_counts: &[usize] = &[1, 4, 16, 64];

    let mut group = c.benchmark_group("concurrency/encode");
    for &workers in worker_counts {
        group.bench_with_input(
            BenchmarkId::new("workers", workers),
            &workers,
            |b, &n| {
                b.to_async(&runtime).iter(|| {
                    let msg = msg.clone();
                    async move {
                        let mut handles = Vec::with_capacity(n);
                        for _ in 0..n {
                            let m = msg.clone();
                            handles.push(tokio::spawn(async move {
                                for _ in 0..100 {
                                    encode(&m).unwrap();
                                }
                            }));
                        }
                        for h in handles {
                            h.await.unwrap();
                        }
                    }
                });
            },
        );
    }
    group.finish();
}

// ── Reassembly with duplicate fragments ──────────────────────────────────────

fn bench_reassembly_with_duplicates(c: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let addr = Address::SocketV4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 9002));
    let data = Bytes::from(vec![3u8; 4800]);
    let chunk = 1200usize;
    // 4 fragments, each duplicated once
    let mut fragments: Vec<UdpPacket> =
        UdpPacket::split_packet(10, addr.clone(), data, chunk, 1).collect();
    let dups = fragments.clone();
    fragments.extend(dups);

    let mut group = c.benchmark_group("concurrency/reassembly_duplicates");
    group.throughput(Throughput::Elements(fragments.len() as u64));
    group.bench_function("4_frags_2x_duplication", |b| {
        b.to_async(&runtime).iter(|| {
            let frags = fragments.clone();
            async move {
                let r = UdpReassembler::default();
                let mut result = None;
                for f in frags {
                    if let Some(r) = r.process(f).await.unwrap() {
                        result = Some(r);
                    }
                }
                assert!(result.is_some());
            }
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_concurrent_sessions,
    bench_concurrent_encode,
    bench_reassembly_with_duplicates,
);
criterion_main!(benches);
