use std::future::Future;
use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub trait Streamable {
    fn write<Stream>(self, stream: &mut Stream) -> impl Future<Output = io::Result<()>> + Send
    where
        Self: Into<Vec<u8>>,
        Stream: AsyncWrite + Unpin + Send,
    {
        let bytes = self.into();
        async move { stream.write_all(&bytes).await }
    }

    fn read<Stream>(stream: &mut Stream) -> impl Future<Output = io::Result<Self>> + Send
    where
        Self: Sized,
        Stream: AsyncRead + Unpin + Send;
}

pub mod util {
    use tokio::sync::broadcast;

    use super::*;

    const DEFAULT_BUF_SIZE: usize = 8 * 1024;

    async fn copy_with_abort<R, W>(
        read: &mut R,
        write: &mut W,
        mut abort: broadcast::Receiver<()>,
    ) -> io::Result<usize>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        use std::io::ErrorKind::{ConnectionAborted, ConnectionReset};

        let mut copied = 0;
        let mut buf = [0u8; DEFAULT_BUF_SIZE];

        loop {
            let bytes_read;

            tokio::select! {
                result = read.read(&mut buf) => {
                    bytes_read = result.or_else(|e| match e.kind() {
                        ConnectionReset | ConnectionAborted => Ok(0),
                        _ => Err(e)
                    })?;
                },
                _ = abort.recv() => {
                    break;
                }
            }

            if bytes_read == 0 {
                break;
            }

            write.write_all(&buf[0..bytes_read]).await?;
            copied += bytes_read;
        }

        Ok(copied)
    }

    pub async fn copy_bidirectional<A, B>(a: &mut A, b: &mut B) -> io::Result<(u64, u64)>
    where
        A: AsyncRead + AsyncWrite + Unpin + ?Sized,
        B: AsyncRead + AsyncWrite + Unpin + ?Sized,
    {
        let (mut a_reader, mut a_writer) = tokio::io::split(a);
        let (mut b_reader, mut b_writer) = tokio::io::split(b);

        let (cancel, _) = broadcast::channel::<()>(1);

        let (a_to_b_bytes, b_to_a_bytes) = tokio::join! {
            async {
                let result = copy_with_abort(&mut a_reader, &mut b_writer, cancel.subscribe()).await;
                let _ = cancel.send(());
                result
            },
            async {
                let result = copy_with_abort(&mut b_reader, &mut a_writer, cancel.subscribe()).await;
                let _ = cancel.send(());
                result
            }
        };

        Ok((
            a_to_b_bytes.unwrap_or(0) as u64,
            b_to_a_bytes.unwrap_or(0) as u64,
        ))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::io;
        use std::pin::Pin;
        use std::task::{Context, Poll};
        use tokio::io::duplex;
        use tokio::sync::broadcast;

        // Mock reader that can be configured to return specific data or errors
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
                buf: &mut tokio::io::ReadBuf<'_>,
            ) -> Poll<io::Result<()>> {
                self.read_count += 1;

                // Check if we should return an error
                if let Some((n, error)) = &self.error_on_nth_read {
                    if self.read_count == *n {
                        return Poll::Ready(Err(io::Error::other(error.to_string())));
                    }
                }

                if self.position >= self.data.len() {
                    buf.set_filled(0);
                    Poll::Ready(Ok(()))
                } else {
                    let remaining = self.data.len() - self.position;
                    let to_read = std::cmp::min(remaining, buf.remaining());
                    buf.put_slice(&self.data[self.position..self.position + to_read]);
                    self.position += to_read;
                    Poll::Ready(Ok(()))
                }
            }
        }

        // Mock writer that stores written data
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

                if let Some((n, error)) = &self.error_on_nth_write {
                    if self.write_count == *n {
                        return Poll::Ready(Err(io::Error::other(error.to_string())));
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

        #[tokio::test]
        async fn test_copy_with_abort_normal() {
            let data = b"Hello, World!".to_vec();
            let mut reader = MockReader::new(data.clone());
            let mut writer = MockWriter::new();
            let (_tx, rx) = broadcast::channel(1);

            let result = copy_with_abort(&mut reader, &mut writer, rx).await.unwrap();

            assert_eq!(result, data.len());
            assert_eq!(writer.written, data);
        }

        #[tokio::test]
        async fn test_copy_with_abort_empty() {
            let mut reader = MockReader::new(vec![]);
            let mut writer = MockWriter::new();
            let (_tx, rx) = broadcast::channel(1);

            let result = copy_with_abort(&mut reader, &mut writer, rx).await.unwrap();

            assert_eq!(result, 0);
            assert!(writer.written.is_empty());
        }

        #[tokio::test]
        async fn test_copy_with_abort_aborted() {
            let data = b"Hello, World!".to_vec();
            let reader = MockReader::new(data);
            let mut writer = MockWriter::new();
            let (tx, rx) = broadcast::channel(1);

            struct DelayedReader<R> {
                inner: R,
                first_read: bool,
            }

            impl<R: AsyncRead + Unpin> AsyncRead for DelayedReader<R> {
                fn poll_read(
                    mut self: Pin<&mut Self>,
                    cx: &mut Context<'_>,
                    buf: &mut tokio::io::ReadBuf<'_>,
                ) -> Poll<io::Result<()>> {
                    if !self.first_read {
                        self.first_read = true;
                        cx.waker().wake_by_ref();
                        return Poll::Pending;
                    }
                    Pin::new(&mut self.inner).poll_read(cx, buf)
                }
            }

            let mut delayed_reader = DelayedReader {
                inner: reader,
                first_read: false,
            };

            tx.send(()).unwrap();

            let result = copy_with_abort(&mut delayed_reader, &mut writer, rx)
                .await
                .unwrap();

            assert_eq!(result, 0, "no data should be copied");
            assert!(
                writer.written.is_empty(),
                "the write buffer should be empty"
            );
        }

        #[tokio::test]
        async fn test_copy_with_abort_after_partial_copy() {
            let data = b"Hello, World!".to_vec();
            let reader = MockReader::new(data);
            let mut writer = MockWriter::new();
            let (tx, rx) = broadcast::channel(1);

            struct CountingReader<R> {
                inner: R,
                read_count: usize,
                tx: broadcast::Sender<()>,
            }

            impl<R: AsyncRead + Unpin> AsyncRead for CountingReader<R> {
                fn poll_read(
                    self: Pin<&mut Self>,
                    cx: &mut Context<'_>,
                    buf: &mut tokio::io::ReadBuf<'_>,
                ) -> Poll<io::Result<()>> {
                    let this = self.get_mut();
                    this.read_count += 1;

                    if this.read_count == 2 {
                        let _ = this.tx.send(());
                    }

                    Pin::new(&mut this.inner).poll_read(cx, buf)
                }
            }

            let mut counting_reader = CountingReader {
                inner: reader,
                read_count: 0,
                tx: tx,
            };

            let result = copy_with_abort(&mut counting_reader, &mut writer, rx)
                .await
                .unwrap();

            assert!(
                result > 0 && result <= 13,
                "part of the data should be copied"
            );
            assert!(
                !writer.written.is_empty(),
                "the write buffer should contain partial data"
            );
        }

        #[tokio::test]
        async fn test_copy_bidirectional_normal() {
            let (mut a_tx, mut a_rx) = duplex(64);
            let (mut b_tx, mut b_rx) = duplex(64);

            let write_handle = tokio::spawn(async move {
                a_tx.write_all(b"Hello").await.unwrap();
                b_tx.write_all(b"World").await.unwrap();

                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

                (a_tx, b_tx)
            });

            let copy_handle =
                tokio::spawn(async move { copy_bidirectional(&mut a_rx, &mut b_rx).await });

            let (a_tx, b_tx) = write_handle.await.unwrap();

            drop(a_tx);
            drop(b_tx);

            let result = copy_handle.await.unwrap().unwrap();

            assert_eq!(result.0, 5); // "Hello"
            assert_eq!(result.1, 5); // "World"
        }

        #[tokio::test]
        async fn test_copy_bidirectional_one_side_empty() {
            let (mut a_tx, mut a_rx) = duplex(64);
            let (b_tx, mut b_rx) = duplex(64);

            let write_handle = tokio::spawn(async move {
                a_tx.write_all(b"Hello").await.unwrap();
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                (a_tx, b_tx)
            });

            let copy_handle =
                tokio::spawn(async move { copy_bidirectional(&mut a_rx, &mut b_rx).await });

            let (a_tx, b_tx) = write_handle.await.unwrap();
            drop(a_tx);
            drop(b_tx);

            let result = copy_handle.await.unwrap().unwrap();

            assert_eq!(result.0, 5); // "Hello"
            assert_eq!(result.1, 0); // Nothing from b to a
        }

        #[tokio::test]
        async fn test_copy_bidirectional_continuous() {
            let (mut a_tx, mut a_rx) = duplex(64);
            let (mut b_tx, mut b_rx) = duplex(64);

            let write_handle = tokio::spawn(async move {
                for _ in 0..5 {
                    a_tx.write_all(b"Hello").await.unwrap();
                    b_tx.write_all(b"World").await.unwrap();
                    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                }
                (a_tx, b_tx)
            });

            let copy_handle =
                tokio::spawn(async move { copy_bidirectional(&mut a_rx, &mut b_rx).await });

            let (a_tx, b_tx) = write_handle.await.unwrap();

            drop(a_tx);
            drop(b_tx);

            let result = copy_handle.await.unwrap().unwrap();

            assert_eq!(result.0, 25); // "Hello" * 5
            assert_eq!(result.1, 25); // "World" * 5
        }

        #[tokio::test]
        async fn test_copy_bidirectional_zero_bytes() {
            let (a_tx, mut a_rx) = duplex(64);
            let (b_tx, mut b_rx) = duplex(64);

            drop(a_tx);
            drop(b_tx);

            let result = copy_bidirectional(&mut a_rx, &mut b_rx).await.unwrap();

            assert_eq!(result.0, 0);
            assert_eq!(result.1, 0);
        }
    }
}
