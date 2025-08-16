use ombrac_transport::quic::client::Builder as QuicClientBuilder;
use ombrac_transport::quic::server::Builder as QuicServerBuilder;

use std::net::SocketAddr;

#[path = "./support.rs"]
mod support;

#[tokio::test(flavor = "multi_thread")]
async fn test_quic_transport() {
    let listen_addr: SocketAddr = "127.0.0.1:50001".parse().unwrap();
    let server_name = "localhost".to_string();

    let mut server_builder = QuicServerBuilder::new(listen_addr);
    server_builder.with_enable_self_signed(true);

    let mut client_builder = QuicClientBuilder::new(listen_addr, server_name);
    client_builder.with_tls_skip(true);

    let server = server_builder.build().await.unwrap();
    let client = client_builder.build().await.unwrap();

    support::run_transport_tests(client, server).await;
}
