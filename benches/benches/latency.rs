use bytes::Bytes;
use criterion::{Criterion, criterion_group, criterion_main};
use ombrac::codec::length_codec;
use ombrac::protocol::{
    Address, ClientConnect, ClientHello, ServerConnectResponse, UdpPacket, decode, encode,
};
use std::net::{Ipv4Addr, SocketAddrV4};
use tokio_util::codec::{Decoder, Encoder};

// ── Per-operation latency ─────────────────────────────────────────────────────

fn bench_encode_latency(c: &mut Criterion) {
    let mut group = c.benchmark_group("latency/encode");

    let hello = ClientHello {
        version: 1,
        secret: [0u8; 32],
        options: Bytes::new(),
    };
    let connect = ClientConnect {
        address: Address::SocketV4(SocketAddrV4::new(Ipv4Addr::new(93, 184, 216, 34), 80)),
    };
    let response_ok = ServerConnectResponse::Ok;
    let response_err = ServerConnectResponse::Err {
        kind: ombrac::protocol::ConnectErrorKind::ConnectionRefused,
        message: "connection refused".to_string(),
    };

    group.bench_function("ClientHello", |b| b.iter(|| encode(&hello).unwrap()));
    group.bench_function("ClientConnect/ipv4", |b| b.iter(|| encode(&connect).unwrap()));
    group.bench_function("ServerConnectResponse/Ok", |b| {
        b.iter(|| encode(&response_ok).unwrap())
    });
    group.bench_function("ServerConnectResponse/Err", |b| {
        b.iter(|| encode(&response_err).unwrap())
    });
    group.finish();
}

fn bench_decode_latency(c: &mut Criterion) {
    let mut group = c.benchmark_group("latency/decode");

    let hello = ClientHello {
        version: 1,
        secret: [0u8; 32],
        options: Bytes::new(),
    };
    let connect = ClientConnect {
        address: Address::SocketV4(SocketAddrV4::new(Ipv4Addr::new(93, 184, 216, 34), 80)),
    };

    let encoded_hello = encode(&hello).unwrap();
    let encoded_connect = encode(&connect).unwrap();

    group.bench_function("ClientHello", |b| {
        b.iter(|| decode::<ClientHello>(&encoded_hello).unwrap())
    });
    group.bench_function("ClientConnect/ipv4", |b| {
        b.iter(|| decode::<ClientConnect>(&encoded_connect).unwrap())
    });
    group.finish();
}

fn bench_roundtrip_latency(c: &mut Criterion) {
    let mut group = c.benchmark_group("latency/roundtrip");

    let connect = ClientConnect {
        address: Address::Domain(Bytes::copy_from_slice(b"example.com"), 443),
    };
    let udp = UdpPacket::Unfragmented {
        session_id: 42,
        address: Address::SocketV4(SocketAddrV4::new(Ipv4Addr::new(8, 8, 8, 8), 53)),
        data: Bytes::from(vec![0u8; 512]),
    };

    group.bench_function("ClientConnect/domain", |b| {
        b.iter(|| {
            let enc = encode(&connect).unwrap();
            decode::<ClientConnect>(&enc).unwrap()
        })
    });
    group.bench_function("UdpPacket/unfragmented_512", |b| {
        b.iter(|| {
            let enc = encode(&udp).unwrap();
            decode::<UdpPacket>(&enc).unwrap()
        })
    });
    group.finish();
}

// ── Codec framing latency ─────────────────────────────────────────────────────

fn bench_codec_frame_latency(c: &mut Criterion) {
    use bytes::BytesMut;

    let mut group = c.benchmark_group("latency/codec_frame");

    let sizes: &[usize] = &[32, 256, 1400, 8192];
    for &size in sizes {
        let payload = Bytes::from(vec![0u8; size]);
        group.bench_function(format!("encode_decode_{size}"), |b| {
            b.iter(|| {
                let mut codec = length_codec();
                let mut buf = BytesMut::new();
                Encoder::<Bytes>::encode(&mut codec, payload.clone(), &mut buf).unwrap();
                codec.decode(&mut buf).unwrap().unwrap()
            })
        });
    }
    group.finish();
}

// ── Address parsing latency ───────────────────────────────────────────────────

fn bench_address_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("latency/address_parse");

    group.bench_function("ipv4", |b| {
        b.iter(|| Address::try_from("93.184.216.34:80").unwrap())
    });
    group.bench_function("ipv6", |b| {
        b.iter(|| Address::try_from("[::1]:443").unwrap())
    });
    group.bench_function("domain", |b| {
        b.iter(|| Address::try_from("example.com:8080").unwrap())
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_encode_latency,
    bench_decode_latency,
    bench_roundtrip_latency,
    bench_codec_frame_latency,
    bench_address_parse,
);
criterion_main!(benches);
