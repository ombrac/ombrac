use criterion::{black_box, criterion_group, criterion_main, Criterion};

use ombrac::request::{Address, Request};

// Benchmark tests using criterion
pub fn request_serialization_benchmark(c: &mut Criterion) {
    let request = Request::TcpConnect(Address::Domain("example.com".to_string(), 80));

    c.bench_function("serialize domain request", |b| {
        b.iter(|| {
            let bytes: Vec<u8> = black_box(request.clone()).into();
            black_box(bytes);
        });
    });
}

criterion_group!(benches, request_serialization_benchmark);
criterion_main!(benches);
