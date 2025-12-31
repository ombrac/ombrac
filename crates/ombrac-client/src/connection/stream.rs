use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// A wrapper around a stream that ensures buffered data is read first.
///
/// This wrapper is used to ensure that any data remaining in the Framed codec's
/// read buffer is consumed before reading from the underlying stream. This prevents
/// data loss when transitioning from framed message-based communication to raw
/// stream communication.
pub struct BufferedStream<S> {
    stream: S,
    buffer: Bytes,
    buffer_pos: usize,
}

impl<S> BufferedStream<S> {
    /// Creates a new `BufferedStream` with optional initial buffer data.
    ///
    /// If `buffer` is provided and non-empty, it will be read first before
    /// any data is read from the underlying `stream`.
    pub fn new(stream: S, buffer: Bytes) -> Self {
        Self {
            stream,
            buffer,
            buffer_pos: 0,
        }
    }

    /// Creates a new `BufferedStream` without any initial buffer.
    ///
    /// This is equivalent to `BufferedStream::new(stream, Bytes::new())`.
    pub fn without_buffer(stream: S) -> Self {
        Self::new(stream, Bytes::new())
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for BufferedStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        // First, try to read from the buffer if there's any remaining data
        if self.buffer_pos < self.buffer.len() {
            let remaining = &self.buffer[self.buffer_pos..];
            let to_copy = remaining.len().min(buf.remaining());

            if to_copy > 0 {
                buf.put_slice(&remaining[..to_copy]);
                self.buffer_pos += to_copy;
            }

            // If we've consumed all buffer data, we can drop it to free memory
            if self.buffer_pos >= self.buffer.len() {
                self.buffer = Bytes::new();
                self.buffer_pos = 0;
            }

            return Poll::Ready(Ok(()));
        }

        // Buffer is exhausted, read from the underlying stream
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for BufferedStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
