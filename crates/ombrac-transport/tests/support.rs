use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

#[cfg(feature = "datagram")]
use ombrac_transport::Unreliable;
use ombrac_transport::{Acceptor, Initiator};

pub async fn run_transport_tests<I, A>(initiator: I, acceptor: A)
where
    I: Initiator,
    A: Acceptor,
{
    let initiator = Arc::new(initiator);
    let acceptor = Arc::new(Mutex::new(acceptor));

    let bi_test_handle = {
        let initiator = Arc::clone(&initiator);
        let acceptor = Arc::clone(&acceptor);
        async move {
            let client_logic = async {
                let mut stream = initiator.open_bidirectional().await.unwrap();

                stream.write_all(b"Hello from initiator!").await.unwrap();
                stream.shutdown().await.unwrap();

                let mut received_data = Vec::new();
                stream.read_to_end(&mut received_data).await.unwrap();

                assert_eq!(b"Hello from acceptor!", &received_data[..]);
            };

            let server_logic = async {
                let mut stream = { acceptor.lock().await.accept_bidirectional().await.unwrap() };

                let mut received_data = Vec::new();
                stream.read_to_end(&mut received_data).await.unwrap();

                assert_eq!(b"Hello from initiator!", &received_data[..]);

                stream.write_all(b"Hello from acceptor!").await.unwrap();
                stream.shutdown().await.unwrap();
            };

            tokio::join!(client_logic, server_logic);
        }
    };
    bi_test_handle.await;

    #[cfg(feature = "datagram")]
    {
        let dgram_test_handle = {
            let initiator = Arc::clone(&initiator);
            let acceptor = Arc::clone(&acceptor);
            async move {
                let client_logic = async {
                    let dgram = initiator.open_datagram().await.unwrap();
                    dgram
                        .send(bytes::Bytes::from_static(b"dgram 1"))
                        .await
                        .unwrap();
                    let bytes = dgram.recv().await.unwrap();
                    assert_eq!(bytes, bytes::Bytes::from_static(b"dgram 2"));
                };

                let server_logic = async {
                    let dgram = acceptor.lock().await.accept_datagram().await.unwrap();
                    let bytes = dgram.recv().await.unwrap();
                    assert_eq!(bytes, bytes::Bytes::from_static(b"dgram 1"));
                    dgram
                        .send(bytes::Bytes::from_static(b"dgram 2"))
                        .await
                        .unwrap();
                };

                tokio::join!(client_logic, server_logic);
            }
        };
        dgram_test_handle.await;
    }
}
