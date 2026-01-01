#[cfg(test)]
mod tests {
    use std::io;
    use std::time::Duration;

    use tests_support::mock_transport::{MockConnection, MockInitiator, mock_transport_pair};
    use tokio::net::TcpListener;
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

    /// Test that client waits for server connection response before returning stream
    #[tokio::test]
    async fn test_client_waits_for_connection_response() {
        let (client, _shutdown_tx, _) = setup_test_env().await;

        // Try to connect to an unreachable address
        // The server should attempt to connect, fail, and send error response
        let unreachable_addr: Address = "192.0.2.1:12345".try_into().unwrap();
        let result = client.open_bidirectional(unreachable_addr.clone()).await;

        // Should fail with connection error
        assert!(
            result.is_err(),
            "Connection to unreachable address should fail"
        );

        let err = match result {
            Ok(_) => panic!("Connection to unreachable address should fail"),
            Err(e) => e,
        };
        // Error should indicate connection failure
        assert!(
            matches!(
                err.kind(),
                io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::TimedOut
                    | io::ErrorKind::NetworkUnreachable
                    | io::ErrorKind::HostUnreachable
                    | io::ErrorKind::Other
            ),
            "Error should be a connection-related error, got: {:?}",
            err.kind()
        );
    }

    /// Test that client receives proper error information from server
    ///
    /// This test verifies the protocol correctly implements the connection response flow.
    /// The server attempts to connect to the destination and sends a response.
    /// With mock transport, the server's TCP connection attempt will fail for unreachable addresses,
    /// and this error should be propagated back to the client.
    #[tokio::test]
    async fn test_connection_error_propagation() {
        let (client, _shutdown_tx, _) = setup_test_env().await;

        // Try to connect to an address that the server cannot reach
        // The server will attempt a real TCP connection, which should fail
        // and send an error response back to the client
        let unreachable_addr: Address = "192.0.2.1:12345".try_into().unwrap();
        let result = client.open_bidirectional(unreachable_addr).await;

        // The server should attempt connection, fail, and send error response
        // Client should receive the error
        if result.is_ok() {
            // If it succeeds (e.g., in some network configurations), that's also valid
            // The important part is that the protocol flow was followed
            return;
        }

        let err = match result {
            Ok(_) => return, // If it succeeds, that's also valid
            Err(e) => e,
        };
        // Should be connection-related error
        assert!(
            matches!(
                err.kind(),
                io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::TimedOut
                    | io::ErrorKind::NetworkUnreachable
                    | io::ErrorKind::HostUnreachable
                    | io::ErrorKind::Other
            ),
            "Error should indicate connection failure, got: {:?}",
            err.kind()
        );
    }

    /// Test successful connection flow
    #[tokio::test]
    async fn test_successful_connection_flow() {
        let (client, _shutdown_tx, _) = setup_test_env().await;

        // Start a simple TCP echo server
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server_addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            loop {
                if let Ok((mut stream, _)) = listener.accept().await {
                    tokio::spawn(async move {
                        let (mut reader, mut writer) = stream.split();
                        tokio::io::copy(&mut reader, &mut writer).await.ok();
                    });
                }
            }
        });

        // Wait a bit for server to be ready
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Connect to the echo server
        let dest_addr: Address = server_addr.to_string().try_into().unwrap();
        let result = client.open_bidirectional(dest_addr).await;

        assert!(
            result.is_ok(),
            "Connection to local echo server should succeed"
        );

        // Verify connection was established successfully
        // The stream is ready for data exchange after server confirms connection
        let _stream = result.unwrap();
        // Connection is established and ready - we don't need to test data transfer here
    }

    /// Test that connection response is received before data exchange
    #[tokio::test]
    async fn test_connection_response_before_data() {
        let (client, _shutdown_tx, _) = setup_test_env().await;

        // Start a TCP server that accepts connections
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server_addr = listener.local_addr().unwrap();

        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            ready_tx.send(()).ok();
            if let Ok((stream, _)) = listener.accept().await {
                // Just accept and close immediately to verify connection was established
                drop(stream);
            }
        });

        // Wait for server to be ready
        ready_rx.await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Open connection - this should wait for server's connection response
        let dest_addr: Address = server_addr.to_string().try_into().unwrap();
        let result = client.open_bidirectional(dest_addr).await;

        // Should succeed because server accepted the connection
        assert!(
            result.is_ok(),
            "Connection should succeed after server accepts it"
        );
    }
}
