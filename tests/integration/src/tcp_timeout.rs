#[cfg(test)]
mod tests {
    use std::io;
    use std::time::Duration;

    use tests_support::mock_transport::{MockConnection, MockInitiator, mock_transport_pair};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
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

    /// Server writes data in two bursts with a delay; all data must be received intact.
    #[tokio::test]
    #[ntest::timeout(30000)]
    async fn test_tcp_slow_server_partial_data() {
        let (client, _shutdown_tx, _) = setup_test_env().await;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server_addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                stream.write_all(b"first_ten!").await.unwrap();
                stream.flush().await.unwrap();
                tokio::time::sleep(Duration::from_millis(200)).await;
                stream.write_all(b"second_ten").await.unwrap();
                // stream drops here, signalling EOF
            }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let dest_addr: Address = server_addr.to_string().try_into().unwrap();
        let mut stream = client.open_bidirectional(dest_addr).await.unwrap();

        let mut buf = vec![0u8; 20];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf[..10], b"first_ten!");
        assert_eq!(&buf[10..], b"second_ten");
    }

    /// Server abruptly drops the connection after writing partial data; client should
    /// observe the bytes that were sent followed by a clean EOF.
    #[tokio::test]
    #[ntest::timeout(30000)]
    async fn test_tcp_server_drops_mid_stream() {
        let (client, _shutdown_tx, _) = setup_test_env().await;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server_addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                stream.write_all(b"partial").await.unwrap();
                stream.flush().await.unwrap();
                drop(stream); // abrupt close
            }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let dest_addr: Address = server_addr.to_string().try_into().unwrap();
        let mut stream = client.open_bidirectional(dest_addr).await.unwrap();

        let mut received = Vec::new();
        let mut tmp = [0u8; 64];
        loop {
            let n = stream.read(&mut tmp).await.unwrap();
            if n == 0 {
                break; // EOF
            }
            received.extend_from_slice(&tmp[..n]);
        }
        assert_eq!(&received, b"partial");
    }

    /// Connecting to a port that is not listening should result in a protocol-level
    /// error (the server returns a connection-refused response to the client).
    #[tokio::test]
    #[ntest::timeout(30000)]
    async fn test_tcp_connection_to_port_not_listening() {
        let (client, _shutdown_tx, _) = setup_test_env().await;

        // Bind a listener, get its port, then drop it so nothing listens.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let dest_addr: Address = format!("127.0.0.1:{port}").try_into().unwrap();
        let result = client.open_bidirectional(dest_addr).await;

        assert!(
            result.is_err(),
            "connection to a port with no listener should fail"
        );
        let err = result.err().unwrap();
        assert!(
            matches!(
                err.kind(),
                io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::Other
                    | io::ErrorKind::BrokenPipe
            ),
            "unexpected error kind: {:?}",
            err.kind()
        );
    }
}
