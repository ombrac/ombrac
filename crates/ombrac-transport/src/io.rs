use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

const DEFAULT_BUF_SIZE: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyBidirectionalStats {
    pub a_to_b_bytes: u64,
    pub b_to_a_bytes: u64,
}

async fn copy_with_cancel<R, W>(
    read: &mut R,
    write: &mut W,
    token: CancellationToken,
) -> Result<usize, (io::Error, usize)>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    use std::io::ErrorKind::{ConnectionAborted, ConnectionReset};

    if token.is_cancelled() {
        return Ok(0);
    }

    let mut copied = 0;
    let mut buf = [0u8; DEFAULT_BUF_SIZE];

    loop {
        let bytes_read;

        tokio::select! {
            biased;
            _ = token.cancelled() => {
                break;
            }
            result = read.read(&mut buf) => {
                let read_result = result.or_else(|e| match e.kind() {
                    ConnectionReset | ConnectionAborted => Ok(0),
                    _ => Err(e)
                });

                match read_result {
                    Ok(n) => bytes_read = n,
                    Err(e) => return Err((e, copied)),
                }
            },
        }

        if bytes_read == 0 {
            break;
        }

        if let Err(e) = write.write_all(&buf[0..bytes_read]).await {
            return Err((e, copied));
        }

        copied += bytes_read;
    }

    Ok(copied)
}

pub async fn copy_bidirectional<A, B>(
    a: &mut A,
    b: &mut B,
) -> Result<CopyBidirectionalStats, (io::Error, CopyBidirectionalStats)>
where
    A: AsyncRead + AsyncWrite + Unpin + ?Sized,
    B: AsyncRead + AsyncWrite + Unpin + ?Sized,
{
    let (mut a_reader, mut a_writer) = tokio::io::split(a);
    let (mut b_reader, mut b_writer) = tokio::io::split(b);

    let token = CancellationToken::new();

    let (a_to_b_result, b_to_a_result) = tokio::join! {
        async {
            let result = copy_with_cancel(&mut a_reader, &mut b_writer, token.clone()).await;
            token.cancel();
            result
        },
        async {
            let result = copy_with_cancel(&mut b_reader, &mut a_writer, token.clone()).await;
            token.cancel();
            result
        }
    };

    let stats = CopyBidirectionalStats {
        a_to_b_bytes: (match a_to_b_result {
            Ok(n) => n,
            Err((_, n)) => n,
        }) as u64,
        b_to_a_bytes: (match b_to_a_result {
            Ok(n) => n,
            Err((_, n)) => n,
        }) as u64,
    };

    if let Err((e, _)) = a_to_b_result {
        return Err((e, stats));
    }

    if let Err((e, _)) = b_to_a_result {
        return Err((e, stats));
    }

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use std::time::Duration;
    use tokio::io::{ReadBuf, duplex};
    use tokio_util::sync::CancellationToken;

    struct MockReader {
        data: Vec<u8>,
        position: usize,
        error_on_nth_read: Option<(usize, io::Error)>,
        read_count: usize,
    }

    impl MockReader {
        fn new(data: Vec<u8>) -> Self {
            Self {
                data,
                position: 0,
                error_on_nth_read: None,
                read_count: 0,
            }
        }
    }

    impl AsyncRead for MockReader {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            self.read_count += 1;

            if let Some((n, e)) = &self.error_on_nth_read {
                if self.read_count == *n {
                    // Create a new error instance to avoid borrowing issues
                    return Poll::Ready(Err(io::Error::new(e.kind(), e.to_string())));
                }
            }

            let remaining = &self.data[self.position..];
            let amt = std::cmp::min(remaining.len(), buf.remaining());
            buf.put_slice(&remaining[..amt]);
            self.position += amt;
            Poll::Ready(Ok(()))
        }
    }

    struct MockWriter {
        written: Vec<u8>,
        error_on_nth_write: Option<(usize, io::Error)>,
        write_count: usize,
    }

    impl MockWriter {
        fn new() -> Self {
            Self {
                written: Vec::new(),
                error_on_nth_write: None,
                write_count: 0,
            }
        }
    }

    impl AsyncWrite for MockWriter {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.write_count += 1;

            if let Some((n, e)) = &self.error_on_nth_write {
                if self.write_count == *n {
                    return Poll::Ready(Err(io::Error::new(e.kind(), e.to_string())));
                }
            }

            self.written.extend_from_slice(buf);
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    // Reusable MockStream for bidirectional tests
    struct MockStream<R, W> {
        reader: R,
        writer: W,
    }
    impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> AsyncRead for MockStream<R, W> {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Pin::new(&mut self.reader).poll_read(cx, buf)
        }
    }
    impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> AsyncWrite for MockStream<R, W> {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            Pin::new(&mut self.writer).poll_write(cx, buf)
        }
        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.writer).poll_flush(cx)
        }
        fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.writer).poll_shutdown(cx)
        }
    }

    // --- Tests for `copy_with_cancel` ---

    #[tokio::test]
    async fn test_copy_with_cancel_normal() {
        let data = b"Hello, World!".to_vec();
        let mut reader = MockReader::new(data.clone());
        let mut writer = MockWriter::new();
        let token = CancellationToken::new();

        let result = copy_with_cancel(&mut reader, &mut writer, token)
            .await
            .unwrap();

        assert_eq!(result, data.len());
        assert_eq!(writer.written, data);
    }

    #[tokio::test]
    async fn test_copy_with_cancel_empty() {
        let mut reader = MockReader::new(vec![]);
        let mut writer = MockWriter::new();
        let token = CancellationToken::new();

        let result = copy_with_cancel(&mut reader, &mut writer, token)
            .await
            .unwrap();

        assert_eq!(result, 0);
        assert!(writer.written.is_empty());
    }

    #[tokio::test]
    async fn test_copy_with_cancel_aborted_immediately() {
        let data = b"Hello, World!".to_vec();
        let mut reader = MockReader::new(data);
        let mut writer = MockWriter::new();
        let token = CancellationToken::new();

        token.cancel();

        let result = copy_with_cancel(&mut reader, &mut writer, token)
            .await
            .unwrap();

        assert_eq!(result, 0);
        assert!(writer.written.is_empty());
    }

    // #[tokio::test]
    // async fn test_copy_with_cancel_after_partial_copy() {
    //     let data = vec![0u8; DEFAULT_BUF_SIZE * 2];
    //     let reader = MockReader::new(data.clone());
    //     let mut writer = MockWriter::new();
    //     let token = CancellationToken::new();

    //     let mut pending_reader = PendingReader {
    //         inner: reader,
    //         slept: false,
    //     };

    //     let task_token = token.clone();
    //     let result_task = tokio::spawn(async move {
    //         let result = copy_with_cancel(&mut pending_reader, &mut writer, task_token).await;
    //         (result, writer)
    //     });

    //     // Yield to allow the copy task to run and enter pending state
    //     tokio::task::yield_now().await;

    //     token.cancel();

    //     let (result, writer) = result_task.await.unwrap();
    //     let result = result.unwrap();

    //     assert_eq!(result, DEFAULT_BUF_SIZE);
    //     assert_eq!(writer.written.len(), DEFAULT_BUF_SIZE);
    //     assert_eq!(&writer.written, &data[..DEFAULT_BUF_SIZE]);
    // }

    #[tokio::test]
    async fn test_copy_with_cancel_read_error_with_stats() {
        let data = vec![0u8; DEFAULT_BUF_SIZE * 2];
        let mut reader = MockReader::new(data);
        reader.error_on_nth_read = Some((2, io::Error::new(io::ErrorKind::Other, "read error")));
        let mut writer = MockWriter::new();
        let token = CancellationToken::new();

        let result = copy_with_cancel(&mut reader, &mut writer, token).await;

        assert!(result.is_err());
        if let Err((err, copied)) = result {
            assert_eq!(err.kind(), io::ErrorKind::Other);
            // Should have successfully copied one chunk before the error on the second read
            assert_eq!(copied, DEFAULT_BUF_SIZE);
            assert_eq!(writer.written.len(), DEFAULT_BUF_SIZE);
        }
    }

    #[tokio::test]
    async fn test_copy_with_cancel_write_error_with_stats() {
        let data = vec![0u8; DEFAULT_BUF_SIZE * 2];
        let mut reader = MockReader::new(data);
        let mut writer = MockWriter::new();
        writer.error_on_nth_write =
            Some((2, io::Error::new(io::ErrorKind::BrokenPipe, "write error")));
        let token = CancellationToken::new();

        let result = copy_with_cancel(&mut reader, &mut writer, token).await;

        assert!(result.is_err());
        if let Err((err, copied)) = result {
            assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
            // Should have successfully copied one chunk before the error on the second write
            assert_eq!(copied, DEFAULT_BUF_SIZE);
            assert_eq!(writer.written.len(), DEFAULT_BUF_SIZE);
        }
    }

    // --- Tests for `copy_bidirectional` ---

    #[tokio::test]
    async fn test_copy_bidirectional_normal() {
        let (mut a, mut a_peer) = duplex(64);
        let (mut b, mut b_peer) = duplex(64);

        let copy_handle = tokio::spawn(async move { copy_bidirectional(&mut a, &mut b).await });

        a_peer.write_all(b"Hello").await.unwrap();
        b_peer.write_all(b"World").await.unwrap();

        // Give some time for the data to be copied
        tokio::time::sleep(Duration::from_millis(10)).await;

        drop(a_peer);
        drop(b_peer);

        let result = copy_handle.await.unwrap().unwrap();

        assert_eq!(
            result,
            CopyBidirectionalStats {
                a_to_b_bytes: 5,
                b_to_a_bytes: 5
            }
        );
    }

    #[tokio::test]
    async fn test_copy_bidirectional_one_side_closes_cancels_other() {
        let (mut a, mut a_peer) = duplex(64);
        let (mut b, b_peer) = duplex(64);

        let copy_handle = tokio::spawn(async move { copy_bidirectional(&mut a, &mut b).await });

        a_peer.write_all(b"data from a").await.unwrap();
        // Give time for the copy to happen before closing
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Closing a_peer will cause a->b copy to finish, which should cancel b->a copy
        drop(a_peer);
        // Closing b_peer ensures the b->a reader doesn't hang forever if cancellation fails
        drop(b_peer);

        let result = copy_handle.await.unwrap().unwrap();

        assert_eq!(
            result,
            CopyBidirectionalStats {
                a_to_b_bytes: 11,
                b_to_a_bytes: 0
            }
        );
    }

    #[tokio::test]
    async fn test_copy_bidirectional_one_side_empty() {
        let (mut a, mut a_peer) = duplex(64);
        let (mut b, b_peer) = duplex(64);

        let copy_handle = tokio::spawn(async move { copy_bidirectional(&mut a, &mut b).await });

        a_peer.write_all(b"Hello").await.unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;

        drop(a_peer);
        drop(b_peer);

        let result = copy_handle.await.unwrap().unwrap();

        assert_eq!(
            result,
            CopyBidirectionalStats {
                a_to_b_bytes: 5,
                b_to_a_bytes: 0
            }
        );
    }

    #[tokio::test]
    async fn test_copy_bidirectional_zero_bytes() {
        let (mut a, a_peer) = duplex(64);
        let (mut b, b_peer) = duplex(64);

        drop(a_peer);
        drop(b_peer);

        let result = copy_bidirectional(&mut a, &mut b).await.unwrap();

        assert_eq!(
            result,
            CopyBidirectionalStats {
                a_to_b_bytes: 0,
                b_to_a_bytes: 0
            }
        );
    }

    #[tokio::test]
    async fn test_copy_bidirectional_write_error_cancels_other() {
        let (mut a, mut a_peer) = duplex(DEFAULT_BUF_SIZE * 2);

        let mut b_writer = MockWriter::new();
        b_writer.error_on_nth_write =
            Some((2, io::Error::new(io::ErrorKind::BrokenPipe, "write error")));
        let mut b = MockStream {
            // Provide some data for the other direction to test cancellation
            reader: MockReader::new(vec![0; DEFAULT_BUF_SIZE * 3]),
            writer: b_writer,
        };

        let copy_handle = tokio::spawn(async move { copy_bidirectional(&mut a, &mut b).await });

        // Write enough data to trigger the error on the second write
        let _ = a_peer.write_all(&vec![0; DEFAULT_BUF_SIZE + 10]).await;
        // Give a moment for the error to propagate and cancel the other direction
        tokio::time::sleep(Duration::from_millis(20)).await;
        // Close the peer to avoid panics on write
        drop(a_peer);

        let result = copy_handle.await.unwrap();

        assert!(result.is_err());
        if let Err((err, stats)) = result {
            assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
            // One chunk was successfully written from a to b
            assert_eq!(stats.a_to_b_bytes, DEFAULT_BUF_SIZE as u64);
            // The b to a copy should have been cancelled before it could copy everything.
            // It might have copied 0 or 1 chunk depending on scheduling.
            assert!(stats.b_to_a_bytes <= DEFAULT_BUF_SIZE as u64);
        }
    }

    #[tokio::test]
    async fn test_copy_bidirectional_read_error_cancels_other() {
        let (mut a, mut a_peer) = duplex(DEFAULT_BUF_SIZE * 2);

        let mut b_reader = MockReader::new(vec![0; DEFAULT_BUF_SIZE * 2]);
        b_reader.error_on_nth_read = Some((2, io::Error::new(io::ErrorKind::Other, "read error")));
        let mut b = MockStream {
            reader: b_reader,
            writer: MockWriter::new(),
        };

        let copy_handle = tokio::spawn(async move { copy_bidirectional(&mut a, &mut b).await });

        // Write some data in the a->b direction
        let _ = a_peer.write_all(b"some data").await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(a_peer);

        let result = copy_handle.await.unwrap();

        assert!(result.is_err());
        if let Err((err, stats)) = result {
            assert_eq!(err.kind(), io::ErrorKind::Other);
            // a->b copy should have completed successfully
            assert_eq!(stats.a_to_b_bytes, 9);
            // b->a copy failed after copying one chunk
            assert_eq!(stats.b_to_a_bytes, DEFAULT_BUF_SIZE as u64);
        }
    }
}
