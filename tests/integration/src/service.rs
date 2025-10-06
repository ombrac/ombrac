#[cfg(test)]
mod tests {
    use std::io;
    use std::time::Duration;

    use tests_support::mock_transport::{MockConnection, MockInitiator, mock_transport_pair};
    use tokio::sync::broadcast;

    use ombrac::protocol::{Address, Secret};
    use ombrac_client::client::Client;
    use ombrac_server::server::Server;

    fn random_secret() -> Secret {
        use rand::RngCore;
        let mut secret = [0u8; 32];
        let mut rng = rand::rng();
        rng.fill_bytes(&mut secret);
        secret
    }

    async fn setup_test_env() -> (
        Client<MockInitiator, MockConnection>,
        broadcast::Sender<()>,
        Secret,
    ) {
        let (initiator, acceptor) = mock_transport_pair();
        let secret = random_secret();

        let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
        tokio::spawn(async move {
            let server = Server::new(acceptor, secret);
            server.accept_loop(shutdown_rx).await.unwrap();
        });

        let client = Client::new(initiator, secret, None).await.unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;

        (client, shutdown_tx, secret)
    }

    #[tokio::test]
    async fn test_handshake_and_tcp_stream_establishment() {
        let (client, _shutdown_tx, _) = setup_test_env().await;

        let dest_addr: Address = "1.2.3.4:80".try_into().unwrap();
        let result = client.open_bidirectional(dest_addr).await;

        assert!(
            result.is_ok(),
            "TCP bidi stream should be established successfully"
        );
    }

    #[tokio::test]
    #[cfg(feature = "datagram")]
    async fn test_udp_proxy_unfragmented() -> io::Result<()> {
        let (client, _shutdown_tx, _) = setup_test_env().await;

        let echo_server = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
        let echo_addr = echo_server.local_addr()?;

        let mut udp_session = client.open_associate();

        let message = bytes::Bytes::from_static(b"hello world");
        let dest_addr: Address = echo_addr.to_string().try_into().unwrap();
        udp_session.send_to(message.clone(), dest_addr).await?;

        let mut buf = [0u8; 1024];
        let (len, from) = echo_server.recv_from(&mut buf).await?;
        assert_eq!(&buf[..len], message.as_ref());
        echo_server.send_to(&buf[..len], from).await?;

        let (response, from_addr) = udp_session.recv_from().await.unwrap();
        assert_eq!(response, message);
        assert_eq!(from_addr.to_string(), echo_addr.to_string());

        Ok(())
    }

    #[tokio::test]
    #[cfg(feature = "datagram")]
    async fn test_udp_proxy_fragmented() -> io::Result<()> {
        let (client, _shutdown_tx, _) = setup_test_env().await;

        let echo_server = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
        let echo_addr = echo_server.local_addr()?;

        let mut udp_session = client.open_associate();

        let mut large_message = Vec::with_capacity(250);
        for i in 0..250 {
            large_message.push(i as u8);
        }
        let large_message = bytes::Bytes::from(large_message);

        let dest_addr: Address = echo_addr.to_string().try_into().unwrap();
        udp_session
            .send_to(large_message.clone(), dest_addr)
            .await?;

        let mut buf = [0u8; 1024];
        let (len, from) = echo_server.recv_from(&mut buf).await?;
        assert_eq!(&buf[..len], large_message.as_ref());
        echo_server.send_to(&buf[..len], from).await?;

        let (response, from_addr) = udp_session.recv_from().await.unwrap();
        assert_eq!(response, large_message);
        assert_eq!(from_addr.to_string(), echo_addr.to_string());

        Ok(())
    }

    #[tokio::test]
    async fn test_handshake_with_invalid_secret() {
        let (initiator, acceptor) = mock_transport_pair();
        let server_secret = random_secret();
        let mut client_secret = random_secret();

        while client_secret == server_secret {
            client_secret = random_secret();
        }

        let (_shutdown_tx, shutdown_rx) = broadcast::channel(1);
        tokio::spawn(async move {
            let server = Server::new(acceptor, server_secret);
            let _ = server.accept_loop(shutdown_rx).await;
        });

        let client_result = Client::new(initiator, client_secret, None).await;

        if let Err(err) = client_result {
            assert_eq!(
                err.kind(),
                io::ErrorKind::PermissionDenied,
                "Error kind should be PermissionDenied for invalid secret"
            );
        } else {
            panic!("Client::new should have failed with an invalid secret, but it succeeded.");
        }
    }

    #[tokio::test]
    async fn test_handshake_with_valid_secret() {
        let (initiator, acceptor) = mock_transport_pair();
        let secret = random_secret();

        let (_shutdown_tx, shutdown_rx) = broadcast::channel(1);
        tokio::spawn(async move {
            let server = Server::new(acceptor, secret);
            let _ = server.accept_loop(shutdown_rx).await;
        });

        // The handshake happens here. We expect it to succeed.
        let client_result = Client::new(initiator, secret, None).await;

        assert!(
            client_result.is_ok(),
            "Client::new should succeed with a valid secret. Error: {:?}",
            client_result.err()
        );
    }
}
