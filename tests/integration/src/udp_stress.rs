#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::Arc;
    use std::time::Duration;

    use bytes::Bytes;
    use tests_support::mock_transport::{MockConnection, MockInitiator, mock_transport_pair};
    use tokio::net::UdpSocket;
    use tokio::sync::broadcast;

    use ombrac::protocol::{Address, Secret};
    use ombrac_client::client::Client;
    use ombrac_server::connection::ConnectionAcceptor;

    fn random_secret() -> Secret {
        use rand::RngCore;
        let mut secret = [0u8; 32];
        rand::rng().fill_bytes(&mut secret);
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

    /// 20 concurrent UDP sessions each send 5 round-trip messages without
    /// data corruption or deadlock.
    #[tokio::test]
    #[ntest::timeout(30000)]
    async fn test_udp_concurrent_sessions() -> io::Result<()> {
        let (client, _shutdown_tx, _) = setup_test_env().await;
        let client = Arc::new(client);

        let echo_server = UdpSocket::bind("127.0.0.1:0").await?;
        let echo_addr = echo_server.local_addr()?;

        // Spawn an echo server that handles all incoming packets.
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            loop {
                match echo_server.recv_from(&mut buf).await {
                    Ok((n, from)) => {
                        let _ = echo_server.send_to(&buf[..n], from).await;
                    }
                    Err(_) => break,
                }
            }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        const SESSIONS: usize = 20;
        const MSGS_PER_SESSION: usize = 5;

        let mut handles = Vec::with_capacity(SESSIONS);
        for session_idx in 0..SESSIONS {
            let client = Arc::clone(&client);
            let dest_addr: Address = echo_addr.to_string().try_into().unwrap();
            handles.push(tokio::spawn(async move {
                let mut session = client.open_associate();
                for msg_idx in 0..MSGS_PER_SESSION {
                    let payload =
                        Bytes::from(format!("session{session_idx}-msg{msg_idx}").into_bytes());
                    session
                        .send_to(payload.clone(), dest_addr.clone())
                        .await
                        .expect("send should succeed");

                    let (received, _) = session
                        .recv_from()
                        .await
                        .expect("channel closed unexpectedly");
                    assert_eq!(
                        received, payload,
                        "session {session_idx} msg {msg_idx}: data mismatch"
                    );
                }
            }));
        }

        for handle in handles {
            handle.await.expect("session task panicked");
        }

        Ok(())
    }

    /// A 2000-byte payload (larger than the 1500-byte mock MTU minus overhead)
    /// must be fragmented, tunneled, and reassembled back to the original bytes.
    ///
    /// MockConnection reports max_datagram_size = Some(1500).
    /// fragmented_overhead() = 277 bytes, so usable payload per fragment = 1223 bytes.
    /// 2000 bytes therefore requires 2 fragments.
    #[tokio::test]
    #[ntest::timeout(30000)]
    async fn test_udp_large_fragmented_roundtrip() -> io::Result<()> {
        let (client, _shutdown_tx, _) = setup_test_env().await;

        let echo_server = UdpSocket::bind("127.0.0.1:0").await?;
        let echo_addr = echo_server.local_addr()?;

        // Echo server task
        tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
            if let Ok((n, from)) = echo_server.recv_from(&mut buf).await {
                let _ = echo_server.send_to(&buf[..n], from).await;
            }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let large_payload: Vec<u8> = (0u16..2000).map(|i| (i % 251) as u8).collect();
        let payload = Bytes::from(large_payload.clone());

        let mut session = client.open_associate();
        let dest_addr: Address = echo_addr.to_string().try_into().unwrap();
        session.send_to(payload, dest_addr).await?;

        let (received, from_addr) = session.recv_from().await.expect("channel closed");
        assert_eq!(
            received.as_ref(),
            large_payload.as_slice(),
            "reassembled payload must match original"
        );
        assert_eq!(from_addr.to_string(), echo_addr.to_string());

        Ok(())
    }
}
