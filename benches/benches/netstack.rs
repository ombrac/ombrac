//! Benchmarks for the userspace netstack used by TUN-mode endpoint.
//!
//! Public-API only (BufferPool is crate-private, so we go through `NetStack::new`).
//! - `IpPacket::new_checked` packet parsing throughput
//! - UDP frame build via `SplitWrite::send`
//! - `NetStack::new` construction cost (worker spawn overhead)

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use bytes::Bytes;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use futures::StreamExt;
use ombrac_netstack::stack::{IpPacket, NetStack, NetStackConfig, Packet};
use ombrac_netstack::udp::UdpPacket as NsUdpPacket;

// ── IP packet parsing ─────────────────────────────────────────────────────────

fn build_ipv4_udp_frame(payload_len: usize) -> Vec<u8> {
    use etherparse::PacketBuilder;
    let builder = PacketBuilder::ipv4([10, 0, 0, 1], [10, 0, 0, 2], 64).udp(1000, 53);
    let payload = vec![0u8; payload_len];
    let mut frame = Vec::with_capacity(builder.size(payload_len));
    builder.write(&mut frame, &payload).unwrap();
    frame
}

fn build_ipv6_udp_frame(payload_len: usize) -> Vec<u8> {
    use etherparse::PacketBuilder;
    let src = [0x20, 0x01, 0xdb, 0x8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
    let dst = [0x20, 0x01, 0xdb, 0x8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2];
    let builder = PacketBuilder::ipv6(src, dst, 64).udp(1000, 53);
    let payload = vec![0u8; payload_len];
    let mut frame = Vec::with_capacity(builder.size(payload_len));
    builder.write(&mut frame, &payload).unwrap();
    frame
}

fn bench_ip_packet_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("netstack/ip_packet_parse");
    let sizes: &[usize] = &[64, 512, 1400];

    for &size in sizes {
        let v4 = build_ipv4_udp_frame(size);
        let v6 = build_ipv6_udp_frame(size);

        group.throughput(Throughput::Bytes(v4.len() as u64));
        group.bench_with_input(BenchmarkId::new("ipv4", size), &v4, |b, frame| {
            b.iter(|| {
                let p = IpPacket::new_checked(frame.as_slice()).unwrap();
                let _ = (p.src_addr(), p.dst_addr(), p.protocol());
            });
        });

        group.bench_with_input(BenchmarkId::new("ipv6", size), &v6, |b, frame| {
            b.iter(|| {
                let p = IpPacket::new_checked(frame.as_slice()).unwrap();
                let _ = (p.src_addr(), p.dst_addr(), p.protocol());
            });
        });
    }
    group.finish();
}

// ── UDP frame build via SplitWrite (full netstack-backed pool) ───────────────

fn bench_udp_send_via_stack(c: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();

    let mut group = c.benchmark_group("netstack/udp_send");
    let sizes: &[usize] = &[64, 512, 1400];

    for &size in sizes {
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            &size,
            |b, &size| {
                b.to_async(&runtime).iter_batched(
                    || {
                        let mut cfg = NetStackConfig::default();
                        cfg.number_workers = 1;
                        cfg.channel_size = 256;
                        let (stack, _tcp, udp) = NetStack::new(cfg);
                        let (sink, source) = stack.split();
                        // Hold sink so the channel stays open; return source + writer.
                        let payload = Bytes::from(vec![0u8; size]);
                        (sink, source, udp, payload)
                    },
                    |(_sink, mut source, udp, payload)| async move {
                        let (_r, mut w) = udp.split();
                        let src = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 1), 1000));
                        let dst =
                            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(8, 8, 8, 8), 53));
                        let pkt = NsUdpPacket {
                            data: Packet::new(payload.clone()),
                            src_addr: src,
                            dst_addr: dst,
                        };
                        w.send(pkt).await.unwrap();
                        let _ = source.next().await;
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

// ── Stack construction cost ──────────────────────────────────────────────────

fn bench_stack_new(c: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();

    let mut group = c.benchmark_group("netstack/construction");
    for &workers in &[1usize, 2, 4] {
        group.bench_with_input(
            BenchmarkId::new("workers", workers),
            &workers,
            |b, &workers| {
                b.to_async(&runtime).iter(|| async move {
                    let mut cfg = NetStackConfig::default();
                    cfg.number_workers = workers;
                    cfg.channel_size = 256;
                    let (_stack, _tcp, _udp) = NetStack::new(cfg);
                });
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_ip_packet_parse,
    bench_udp_send_via_stack,
    bench_stack_new,
);
criterion_main!(benches);
