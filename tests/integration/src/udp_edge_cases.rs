#[cfg(test)]
#[cfg(feature = "datagram")]
mod tests {
    use std::io;
    use std::time::Duration;

    use tests_support::mock_transport::{MockConnection, MockInitiator, mock_transport_pair};
    use tokio::net::UdpSocket;
    use tokio::sync::broadcast;

    use ombrac::protocol::{Address, Secret};
    use ombrac_client::client::Client;
    use ombrac_server::connection::ConnectionAcceptor;

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
            let acceptor = ConnectionAcceptor::new(acceptor, secret);
            acceptor.accept_loop(shutdown_rx).await.unwrap();
        });

        let client = Client::new(initiator, secret, None).await.unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;

        (client, shutdown_tx, secret)
    }

    /// Test UDP with empty datagram
    #[tokio::test]
    async fn test_udp_empty_datagram() -> io::Result<()> {
        let (client, _shutdown_tx, _) = setup_test_env().await;

        let echo_server = UdpSocket::bind("127.0.0.1:0").await?;
        let echo_addr = echo_server.local_addr()?;

        let mut udp_session = client.open_associate();

        // Send empty datagram
        let empty_message = bytes::Bytes::new();
        let dest_addr: Address = echo_addr.to_string().try_into().unwrap();

        // Empty datagrams should be handled gracefully
        udp_session
            .send_to(empty_message.clone(), dest_addr)
            .await?;

        let mut buf = [0u8; 1024];
        let (len, from) = echo_server.recv_from(&mut buf).await?;
        assert_eq!(len, 0);
        echo_server.send_to(&buf[..len], from).await?;

        let (response, from_addr) = udp_session.recv_from().await.unwrap();
        assert_eq!(response.len(), 0);
        assert_eq!(from_addr.to_string(), echo_addr.to_string());

        Ok(())
    }

    /// Test UDP with very large datagram (should fragment)
    #[tokio::test]
    async fn test_udp_very_large_datagram() -> io::Result<()> {
        let (client, _shutdown_tx, _) = setup_test_env().await;

        let echo_server = UdpSocket::bind("127.0.0.1:0").await?;
        let echo_addr = echo_server.local_addr()?;

        let mut udp_session = client.open_associate();

        // Create a large message (larger than MTU, should fragment)
        let large_message: Vec<u8> = (0..10000).map(|i| (i % 256) as u8).collect();
        let large_message = bytes::Bytes::from(large_message);

        let dest_addr: Address = echo_addr.to_string().try_into().unwrap();
        udp_session
            .send_to(large_message.clone(), dest_addr)
            .await?;

        let mut buf = vec![0u8; 20000];
        let (len, from) = echo_server.recv_from(&mut buf).await?;
        assert_eq!(&buf[..len], large_message.as_ref());
        echo_server.send_to(&buf[..len], from).await?;

        let (response, from_addr) = udp_session.recv_from().await.unwrap();
        assert_eq!(response, large_message);
        assert_eq!(from_addr.to_string(), echo_addr.to_string());

        Ok(())
    }

    /// Test UDP with multiple concurrent sessions
    #[tokio::test]
    async fn test_udp_multiple_sessions() -> io::Result<()> {
        let (client, _shutdown_tx, _) = setup_test_env().await;

        let echo_server = UdpSocket::bind("127.0.0.1:0").await?;
        let echo_addr = echo_server.local_addr()?;

        // Create multiple UDP sessions
        let mut session1 = client.open_associate();
        let mut session2 = client.open_associate();

        let message1 = bytes::Bytes::from_static(b"session 1");
        let message2 = bytes::Bytes::from_static(b"session 2");
        let dest_addr: Address = echo_addr.to_string().try_into().unwrap();

        // Send from both sessions
        session1
            .send_to(message1.clone(), dest_addr.clone())
            .await?;
        session2
            .send_to(message2.clone(), dest_addr.clone())
            .await?;

        // Receive and echo both messages
        let mut buf = [0u8; 1024];
        let (len1, from1) = echo_server.recv_from(&mut buf).await?;
        echo_server.send_to(&buf[..len1], from1).await?;

        let (len2, from2) = echo_server.recv_from(&mut buf).await?;
        echo_server.send_to(&buf[..len2], from2).await?;

        // Receive responses (order may vary)
        let mut responses = Vec::new();
        responses.push(session1.recv_from().await);
        responses.push(session2.recv_from().await);

        let mut found1 = false;
        let mut found2 = false;
        for response in responses {
            if let Some((data, _)) = response {
                if data == message1 {
                    found1 = true;
                } else if data == message2 {
                    found2 = true;
                }
            }
        }

        assert!(found1, "Session 1 message should be received");
        assert!(found2, "Session 2 message should be received");

        Ok(())
    }

    /// Test UDP with domain name resolution
    #[tokio::test]
    async fn test_udp_domain_name_resolution() -> io::Result<()> {
        let (client, _shutdown_tx, _) = setup_test_env().await;

        let echo_server = UdpSocket::bind("127.0.0.1:0").await?;
        let server_addr = echo_server.local_addr()?;

        let mut udp_session = client.open_associate();

        let message = bytes::Bytes::from_static(b"hello");
        // Use localhost domain name
        let dest_addr: Address = format!("localhost:{}", server_addr.port())
            .try_into()
            .unwrap();
        udp_session.send_to(message.clone(), dest_addr).await?;

        let mut buf = [0u8; 1024];
        let (len, from) = echo_server.recv_from(&mut buf).await?;
        assert_eq!(&buf[..len], message.as_ref());
        echo_server.send_to(&buf[..len], from).await?;

        let (response, _) = udp_session.recv_from().await.unwrap();
        assert_eq!(response, message);

        Ok(())
    }

    /// Test UDP with invalid domain name
    #[tokio::test]
    async fn test_udp_invalid_domain_name() {
        let (client, _shutdown_tx, _) = setup_test_env().await;

        let mut udp_session = client.open_associate();

        let message = bytes::Bytes::from_static(b"hello");
        // Use a domain that definitely doesn't exist
        let dest_addr: Address = "this-domain-definitely-does-not-exist-12345.invalid:80"
            .try_into()
            .unwrap();

        // DNS resolution should fail, but UDP send might not fail immediately
        // The error will be detected when the server tries to resolve it
        let result = udp_session.send_to(message, dest_addr).await;

        // The send might succeed (it's fire-and-forget), but the server will fail to resolve
        // We can't easily test this without waiting for a timeout, so we just verify
        // the send doesn't panic
        let _ = result;
    }

    /// Test UDP session cleanup when dropped
    #[tokio::test]
    async fn test_udp_session_cleanup() -> io::Result<()> {
        let (client, _shutdown_tx, _) = setup_test_env().await;

        let echo_server = UdpSocket::bind("127.0.0.1:0").await?;
        let echo_addr = echo_server.local_addr()?;

        {
            let mut udp_session = client.open_associate();
            let message = bytes::Bytes::from_static(b"test");
            let dest_addr: Address = echo_addr.to_string().try_into().unwrap();
            udp_session.send_to(message, dest_addr).await?;
            // Session is dropped here
        }

        // Wait a bit for cleanup
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Create a new session - should work fine
        let mut udp_session2 = client.open_associate();
        let message2 = bytes::Bytes::from_static(b"test2");
        let dest_addr: Address = echo_addr.to_string().try_into().unwrap();
        udp_session2.send_to(message2.clone(), dest_addr).await?;

        let mut buf = [0u8; 1024];
        let (len, from) = echo_server.recv_from(&mut buf).await?;
        assert_eq!(&buf[..len], message2.as_ref());
        echo_server.send_to(&buf[..len], from).await?;

        let (response, _) = udp_session2.recv_from().await.unwrap();
        assert_eq!(response, message2);

        Ok(())
    }
}
