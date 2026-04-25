use bytes::Bytes;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use ombrac::protocol::{Address, ClientConnect, ClientHello, UdpPacket, decode, encode};
use std::net::{Ipv4Addr, SocketAddrV4};

// ── Serialization throughput ─────────────────────────────────────────────────

fn bench_encode_client_hello(c: &mut Criterion) {
    let msg = ClientHello {
        version: 1,
        secret: [0xab; 32],
        options: Bytes::new(),
    };

    c.bench_function("encode/ClientHello", |b| {
        b.iter(|| encode(&msg).unwrap());
    });
}

fn bench_encode_client_connect(c: &mut Criterion) {
    let mut group = c.benchmark_group("encode/ClientConnect");

    let ipv4 = ClientConnect {
        address: Address::SocketV4(SocketAddrV4::new(Ipv4Addr::new(1, 2, 3, 4), 443)),
    };
    let domain = ClientConnect {
        address: Address::Domain(Bytes::copy_from_slice(b"example.com"), 443),
    };

    group.bench_function("ipv4", |b| b.iter(|| encode(&ipv4).unwrap()));
    group.bench_function("domain", |b| b.iter(|| encode(&domain).unwrap()));
    group.finish();
}

fn bench_encode_decode_udp_unfragmented(c: &mut Criterion) {
    let sizes: &[usize] = &[64, 512, 1400, 8192, 65535];
    let addr = Address::SocketV4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 53));

    let mut group = c.benchmark_group("udp/unfragmented");
    for &size in sizes {
        let payload = Bytes::from(vec![0u8; size]);
        let pkt = UdpPacket::Unfragmented {
            session_id: 1,
            address: addr.clone(),
            data: payload.clone(),
        };
        let encoded = encode(&pkt).unwrap();

        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::new("encode", size), &pkt, |b, p| {
            b.iter(|| encode(p).unwrap());
        });
        group.bench_with_input(BenchmarkId::new("decode", size), &encoded, |b, e| {
            b.iter(|| decode::<UdpPacket>(e).unwrap());
        });
    }
    group.finish();
}

fn bench_udp_split_packet(c: &mut Criterion) {
    let addr = Address::SocketV4(SocketAddrV4::new(Ipv4Addr::new(1, 1, 1, 1), 8080));
    let sizes: &[usize] = &[1500, 16384, 65535];
    let fragment_payload = 1200usize;

    let mut group = c.benchmark_group("udp/split_packet");
    for &size in sizes {
        let data = Bytes::from(vec![0u8; size]);
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &data, |b, d| {
            b.iter(|| {
                UdpPacket::split_packet(1, addr.clone(), d.clone(), fragment_payload, 0)
                    .for_each(|_| {});
            });
        });
    }
    group.finish();
}

// ── UDP reassembly throughput ─────────────────────────────────────────────────

fn bench_reassembly_in_order(c: &mut Criterion) {
    use ombrac::reassembly::UdpReassembler;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let addr = Address::SocketV4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 9000));
    let sizes: &[usize] = &[1024, 16384, 65535];
    let chunk = 1200usize;

    let mut group = c.benchmark_group("reassembly/in_order");
    for &total in sizes {
        let data = Bytes::from(vec![1u8; total]);
        let fragments: Vec<UdpPacket> = UdpPacket::split_packet(1, addr.clone(), data, chunk, 0)
            .collect();

        group.throughput(Throughput::Bytes(total as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(total),
            &fragments,
            |b, frags| {
                b.to_async(&runtime).iter(|| async {
                    let r = UdpReassembler::default();
                    let mut result = None;
                    for f in frags {
                        result = r.process(f.clone()).await.unwrap();
                    }
                    assert!(result.is_some());
                });
            },
        );
    }
    group.finish();
}

fn bench_reassembly_out_of_order(c: &mut Criterion) {
    use ombrac::reassembly::UdpReassembler;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let addr = Address::SocketV4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 9001));
    let total = 16384usize;
    let chunk = 1200usize;
    let data = Bytes::from(vec![2u8; total]);
    let mut fragments: Vec<UdpPacket> =
        UdpPacket::split_packet(2, addr.clone(), data, chunk, 0).collect();
    // reverse order
    fragments.reverse();

    let mut group = c.benchmark_group("reassembly/out_of_order");
    group.throughput(Throughput::Bytes(total as u64));
    group.bench_function("16384_reversed", |b| {
        b.to_async(&runtime).iter(|| async {
            let r = UdpReassembler::default();
            let mut result = None;
            for f in &fragments {
                result = r.process(f.clone()).await.unwrap();
            }
            assert!(result.is_some());
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_encode_client_hello,
    bench_encode_client_connect,
    bench_encode_decode_udp_unfragmented,
    bench_udp_split_packet,
    bench_reassembly_in_order,
    bench_reassembly_out_of_order,
);
criterion_main!(benches);
