#[cfg(test)]
mod tests_transport_quic {
    use std::time::Duration;

    use ombrac_client::Client;
    use ombrac_server::Server;
    use ombrac_transport::quic::{
        client::Builder as QuicClientBuilder, server::Builder as QuicServerBuilder,
    };
    use tests_support::net::{tcp::ResponseTcpServer, *};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn test_transport_quic_tcp() {
        let secret = [0u8; 32];
        let server_addr = find_available_local_udp_addr();

        let tcp_server = ResponseTcpServer::new().await.unwrap();
        tcp_server
            .set_response(b"test_request".to_vec(), b"test_response".to_vec())
            .await;

        let tcp_server_handle = tcp_server.start().await.unwrap();

        tokio::spawn(async move {
            let server = QuicServerBuilder::new(server_addr)
                .with_tls_skip(true)
                .build()
                .await
                .unwrap();
            Server::new(secret, server).listen().await.unwrap();
        });

        tokio::time::sleep(Duration::from_millis(500)).await;

        let client = QuicClientBuilder::new(server_addr, "localhost")
            .with_tls_skip(true)
            .build()
            .await
            .unwrap();
        let client = Client::new(secret, client);

        let mut stream = client.connect(tcp_server_handle.addr()).await.unwrap();

        stream.write_all(b"test_request").await.unwrap();
        stream.flush().await.unwrap();

        let mut buffer = vec![0; 1024];
        let n = stream.read(&mut buffer).await.unwrap();

        assert_eq!(&buffer[..n], b"test_response");
    }

    #[cfg(feature = "datagram")]
    #[tokio::test]
    async fn test_transport_quic_udp() {
        use tests_support::net::udp::ResponseUdpServer;

        let secret = [0u8; 32];

        let udp_server = ResponseUdpServer::new();

        udp_server
            .set_response(b"test_request".to_vec(), b"test_response".to_vec())
            .await;

        let udp_server_handle = udp_server.start().await.unwrap();
        let udp_addr = udp_server_handle.addr();

        let server_addr = find_available_local_udp_addr();

        tokio::spawn(async move {
            let server = QuicServerBuilder::new(server_addr)
                .with_tls_skip(true)
                .build()
                .await
                .unwrap();

            Server::new(secret, server).listen().await.unwrap();
        });

        tokio::time::sleep(Duration::from_millis(500)).await;

        let client = QuicClientBuilder::new(server_addr, "localhost")
            .with_tls_skip(true)
            .build()
            .await
            .unwrap();
        let client = Client::new(secret, client);

        let data = b"test_request".to_vec();
        let stream = client.associate().await.unwrap();

        stream.send(data, udp_addr).await.unwrap();

        let (bytes, _addr) = stream.recv().await.unwrap();

        assert_eq!(&bytes.to_vec(), b"test_response");
    }
}
