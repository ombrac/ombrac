use std::net::UdpSocket;

use ombrac_transport::quic::client::{Client, Config as ClientConfig};
use ombrac_transport::quic::server::{Config as ServerConfig, Server};

#[path = "./support.rs"]
mod support;

#[tokio::test(flavor = "multi_thread")]
async fn test_quic_transport() {
    let server_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let client_socket = UdpSocket::bind("127.0.0.1:0").unwrap();

    let mut server_config = ServerConfig::default();
    server_config.enable_self_signed = true;
    let server_addr = server_socket.local_addr().unwrap();
    let server = Server::new(server_config, server_socket).await.unwrap();

    let mut client_config = ClientConfig::new(server_addr, "localhost".to_string());
    client_config.skip_server_verification = true;
    let client = Client::new(client_config, client_socket).await.unwrap();

    support::run_transport_tests(client, server).await;
}
