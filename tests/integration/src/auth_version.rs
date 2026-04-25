#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bytes::Bytes;
    use tests_support::mock_transport::mock_transport_pair;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::broadcast;

    use ombrac::codec::ClientMessage;
    use ombrac::protocol::{ClientHello, PROTOCOL_VERSION, Secret, ServerAuthResponse, encode};
    use ombrac_server::connection::ConnectionAcceptor;
    use ombrac_transport::{Connection, Initiator};

    fn random_secret() -> Secret {
        use rand::RngCore;
        let mut secret = [0u8; 32];
        rand::rng().fill_bytes(&mut secret);
        secret
    }

    /// Write a single length-delimited frame (4-byte big-endian length + payload).
    async fn write_frame<W: AsyncWriteExt + Unpin>(w: &mut W, payload: &[u8]) {
        let len = payload.len() as u32;
        w.write_all(&len.to_be_bytes()).await.unwrap();
        w.write_all(payload).await.unwrap();
        w.flush().await.unwrap();
    }

    /// Read a single length-delimited frame.
    async fn read_frame<R: AsyncReadExt + Unpin>(r: &mut R) -> Vec<u8> {
        let mut len_buf = [0u8; 4];
        r.read_exact(&mut len_buf).await.unwrap();
        let len = u32::from_be_bytes(len_buf) as usize;
        let mut payload = vec![0u8; len];
        r.read_exact(&mut payload).await.unwrap();
        payload
    }

    /// A wrong protocol version should result in the server closing the stream
    /// (either an error or EOF — no `Ok` auth response).
    #[tokio::test]
    #[ntest::timeout(30000)]
    async fn test_handshake_wrong_version_rejected() {
        let (initiator, acceptor) = mock_transport_pair();
        let secret = random_secret();

        let (_shutdown_tx, shutdown_rx) = broadcast::channel(1);
        tokio::spawn(async move {
            let acceptor = ConnectionAcceptor::new(acceptor, secret);
            let _ = acceptor.accept_loop(shutdown_rx).await;
        });

        // Connect at the raw transport level and send a wrong-version hello.
        let conn = initiator.connect().await.unwrap();
        let mut stream = Connection::open_bidirectional(&conn).await.unwrap();

        let bad_hello = ClientMessage::Hello(ClientHello {
            version: 0x00, // wrong version
            secret,
            options: Bytes::new(),
        });
        let payload = encode(&bad_hello).unwrap();
        write_frame(&mut stream, &payload).await;

        tokio::time::sleep(Duration::from_millis(200)).await;

        // The server should either close the stream (EOF) or send an Err frame.
        // In either case, it must NOT send ServerAuthResponse::Ok.
        let mut tmp = [0u8; 128];
        let n = stream.read(&mut tmp).await.unwrap_or(0);

        if n >= 5 {
            // Try to decode what the server sent.
            let frame_len = u32::from_be_bytes([tmp[0], tmp[1], tmp[2], tmp[3]]) as usize;
            if n >= 4 + frame_len {
                let body = &tmp[4..4 + frame_len];
                if let Ok(resp) = ombrac::protocol::decode::<ServerAuthResponse>(body) {
                    assert_ne!(
                        resp,
                        ServerAuthResponse::Ok,
                        "server must not accept a wrong-version client"
                    );
                }
            }
        }
        // If n == 0 the server closed the stream — also acceptable.
    }

    /// The correct version with a matching secret must result in
    /// `ServerAuthResponse::Ok`.
    #[tokio::test]
    #[ntest::timeout(30000)]
    async fn test_handshake_correct_version_manual() {
        let (initiator, acceptor) = mock_transport_pair();
        let secret = random_secret();

        let (_shutdown_tx, shutdown_rx) = broadcast::channel(1);
        tokio::spawn(async move {
            let acceptor = ConnectionAcceptor::new(acceptor, secret);
            let _ = acceptor.accept_loop(shutdown_rx).await;
        });

        let conn = initiator.connect().await.unwrap();
        let mut stream = Connection::open_bidirectional(&conn).await.unwrap();

        let hello = ClientMessage::Hello(ClientHello {
            version: PROTOCOL_VERSION,
            secret,
            options: Bytes::new(),
        });
        let payload = encode(&hello).unwrap();
        write_frame(&mut stream, &payload).await;

        // Read the server's auth response frame.
        let response_bytes = read_frame(&mut stream).await;
        let resp: ServerAuthResponse =
            ombrac::protocol::decode(&response_bytes).expect("server should send a valid response");

        assert_eq!(
            resp,
            ServerAuthResponse::Ok,
            "correct version + secret should be accepted"
        );
    }
}
