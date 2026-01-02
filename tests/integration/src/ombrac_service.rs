#[cfg(test)]
mod tests {
    use std::io;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::time::Duration;

    use std::error::Error;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use ombrac::protocol::Address;
    use ombrac_client::{
        OmbracClient, ServiceConfig as ClientServiceConfig,
        TransportConfig as ClientTransportConfig,
    };
    use ombrac_server::{
        OmbracServer, ServiceConfig as ServerServiceConfig,
        TransportConfig as ServerTransportConfig, config::TlsMode as ServerTlsMode,
    };

    fn random_secret() -> String {
        use rand::RngCore;
        let mut secret = [0u8; 32];
        let mut rng = rand::rng();
        rng.fill_bytes(&mut secret);
        // Use a simple hex-like encoding for the secret string
        secret.iter().map(|b| format!("{:02x}", b)).collect()
    }

    #[tokio::test]
    #[ntest::timeout(30000)]
    async fn test_ombrac_server_build() {
        let secret = random_secret();
        let server_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();

        let server_config = Arc::new(ServerServiceConfig {
            secret: secret.clone(),
            listen: server_addr,
            transport: ServerTransportConfig {
                tls_mode: Some(ServerTlsMode::Insecure),
                ..Default::default()
            },
            connection: Default::default(),
            logging: Default::default(),
        });

        let server_result = OmbracServer::build(server_config).await;
        assert!(server_result.is_ok(), "OmbracServer::build should succeed");

        let server = server_result.unwrap();
        server.shutdown().await;
    }

    // Helper function to get an available UDP port
    async fn get_available_port() -> u16 {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        socket.local_addr().unwrap().port()
    }

    // Helper function to setup server and client for testing
    async fn setup_test_env() -> (OmbracClient, OmbracServer, String, SocketAddr) {
        let secret = random_secret();

        // Get an available port and let OmbracServer bind to it
        let port = get_available_port().await;
        let server_addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();

        let server_config = Arc::new(ServerServiceConfig {
            secret: secret.clone(),
            listen: server_addr,
            transport: ServerTransportConfig {
                tls_mode: Some(ServerTlsMode::Insecure),
                ..Default::default()
            },
            connection: Default::default(),
            logging: Default::default(),
        });

        let server = OmbracServer::build(server_config).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Create OmbracClient
        let client_config = Arc::new(ClientServiceConfig {
            secret: secret.clone(),
            server: format!("127.0.0.1:{}", port),
            auth_option: None,
            endpoint: ombrac_client::config::EndpointConfig {
                socks: Some("127.0.0.1:0".parse().unwrap()),
                ..Default::default()
            },
            transport: ClientTransportConfig {
                tls_mode: Some(ombrac_client::config::TlsMode::Insecure),
                ..Default::default()
            },
            logging: Default::default(),
        });

        let client = OmbracClient::build(client_config).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;

        (client, server, secret, server_addr)
    }

    #[tokio::test]
    #[ntest::timeout(30000)]
    async fn test_ombrac_client_build() {
        // First start a server - let OmbracServer bind the port itself
        let secret = random_secret();
        let port = get_available_port().await;
        let server_addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();

        let server_config = Arc::new(ServerServiceConfig {
            secret: secret.clone(),
            listen: server_addr,
            transport: ServerTransportConfig {
                tls_mode: Some(ServerTlsMode::Insecure),
                ..Default::default()
            },
            connection: Default::default(),
            logging: Default::default(),
        });

        let server = OmbracServer::build(server_config).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Now create client
        let client_config = Arc::new(ClientServiceConfig {
            secret: secret.clone(),
            server: format!("127.0.0.1:{}", port),
            auth_option: None,
            endpoint: ombrac_client::config::EndpointConfig {
                socks: Some("127.0.0.1:0".parse().unwrap()),
                ..Default::default()
            },
            transport: ClientTransportConfig {
                tls_mode: Some(ombrac_client::config::TlsMode::Insecure),
                ..Default::default()
            },
            logging: Default::default(),
        });

        let client_result = OmbracClient::build(client_config).await;

        // Cleanup
        server.shutdown().await;

        // Client build should succeed with SOCKS endpoint
        assert!(
            client_result.is_ok(),
            "OmbracClient::build should succeed with SOCKS endpoint"
        );
        if let Ok(client) = client_result {
            client.shutdown().await;
        }
    }

    #[tokio::test]
    #[ntest::timeout(30000)]
    async fn test_udp_proxy_with_real_transport() -> io::Result<()> {
        let (ombrac_client, server, _secret, _server_addr) = setup_test_env().await;

        // Get the internal Client from OmbracClient
        let client = ombrac_client.client();

        // Test UDP proxy
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

        ombrac_client.shutdown().await;
        server.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    #[ntest::timeout(30000)]
    async fn test_tcp_proxy_with_real_transport() -> io::Result<()> {
        let (ombrac_client, server, _secret, _server_addr) = setup_test_env().await;

        // Get the internal Client from OmbracClient
        let client = ombrac_client.client();

        // Start echo server
        let echo_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let echo_addr = echo_listener.local_addr()?;

        tokio::spawn(async move {
            loop {
                match echo_listener.accept().await {
                    Ok((mut stream, _)) => {
                        tokio::spawn(async move {
                            let (mut reader, mut writer) = stream.split();
                            let _ = tokio::io::copy(&mut reader, &mut writer).await;
                        });
                    }
                    Err(_) => break,
                }
            }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        // Test TCP proxy
        let dest_addr: Address = echo_addr.to_string().try_into().unwrap();
        let mut stream = client.open_bidirectional(dest_addr).await?;

        let message = b"hello world";
        stream.write_all(message).await?;
        stream.flush().await?;

        let mut buf = [0u8; 1024];
        let len = stream.read(&mut buf).await?;
        assert_eq!(&buf[..len], message);

        ombrac_client.shutdown().await;
        server.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    #[ntest::timeout(30000)]
    async fn test_handshake_with_invalid_secret() {
        let secret = random_secret();
        let mut wrong_secret = random_secret();

        while wrong_secret == secret {
            wrong_secret = random_secret();
        }

        // Start server - let OmbracServer bind the port itself
        let port = get_available_port().await;
        let server_addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();

        let server_config = Arc::new(ServerServiceConfig {
            secret: secret.clone(),
            listen: server_addr,
            transport: ServerTransportConfig {
                tls_mode: Some(ServerTlsMode::Insecure),
                ..Default::default()
            },
            connection: Default::default(),
            logging: Default::default(),
        });

        let server = OmbracServer::build(server_config).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Try to create client with wrong secret
        let client_config = Arc::new(ClientServiceConfig {
            secret: wrong_secret.clone(),
            server: format!("127.0.0.1:{}", port),
            auth_option: None,
            endpoint: ombrac_client::config::EndpointConfig {
                socks: Some("127.0.0.1:0".parse().unwrap()),
                ..Default::default()
            },
            transport: ClientTransportConfig {
                tls_mode: Some(ombrac_client::config::TlsMode::Insecure),
                ..Default::default()
            },
            logging: Default::default(),
        });

        let client_result = OmbracClient::build(client_config).await;

        server.shutdown().await;

        assert!(
            client_result.is_err(),
            "OmbracClient should fail with wrong secret"
        );
        // The error might be from OmbracClient::build or from the underlying connection
        if let Err(err) = client_result {
            // Check if it's a service error that contains an IO error
            if let Some(io_err) = err.source().and_then(|e| e.downcast_ref::<io::Error>()) {
                match io_err.kind() {
                    io::ErrorKind::PermissionDenied => {}
                    io::ErrorKind::ConnectionReset | io::ErrorKind::UnexpectedEof => {}
                    _ => {
                        // Also check for Quic errors which might indicate authentication failure
                        eprintln!("Error details: {:?}", err);
                    }
                }
            }
        }
    }

    #[tokio::test]
    #[ntest::timeout(30000)]
    async fn test_handshake_with_valid_secret() {
        let (_ombrac_client, server, _secret, _server_addr) = setup_test_env().await;

        // The setup_test_env already created a client with valid secret, so if we got here, it succeeded
        _ombrac_client.shutdown().await;
        server.shutdown().await;
    }
}
