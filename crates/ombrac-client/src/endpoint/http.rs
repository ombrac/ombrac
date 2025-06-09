use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, combinators::BoxBody};
use hyper::{Method, Request, Response};
use tokio::net::TcpListener;

use ombrac::prelude::Address;
use ombrac_macros::{debug, error, info};
use ombrac_transport::Initiator;

use crate::Client;

type ClientBuilder = hyper::client::conn::http1::Builder;
type ServerBuilder = hyper::server::conn::http1::Builder;

pub struct Server<T: Initiator>(TcpListener, Arc<Client<T>>);

impl<T: Initiator> Server<T> {
    pub async fn bind<A: Into<SocketAddr>>(addr: A, ombrac: Arc<Client<T>>) -> io::Result<Self> {
        let inner = TcpListener::bind(addr.into()).await?;
        Ok(Self(inner, ombrac))
    }

    pub async fn listen(&self) -> io::Result<()> {
        let ombrac = Arc::clone(&self.1);

        info!("HTTP/HTTPS Server Listening on {}", self.0.local_addr()?);

        loop {
            match self.0.accept().await {
                Ok((stream, _addr)) => {
                    let ombrac = ombrac.clone();

                    tokio::spawn(async move {
                        let io = TokioIo::new(stream);
                        if let Err(_error) = ServerBuilder::new()
                            .preserve_header_case(true)
                            .title_case_headers(true)
                            .serve_connection(
                                io,
                                hyper::service::service_fn(|req| async {
                                    Self::tunnel(req, ombrac.clone()).await
                                }),
                            )
                            .with_upgrades()
                            .await
                        {
                            error!("Failed to serve connection: {}", _error);
                        }
                    });
                }

                Err(_error) => {
                    error!("Failed to accept: {}", _error);
                    continue;
                }
            }
        }
    }

    async fn tunnel(
        req: Request<hyper::body::Incoming>,
        conn: Arc<Client<T>>,
    ) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
        use ombrac::io::util::copy_bidirectional;

        let host = match req.uri().host() {
            Some(addr) => addr,
            None => {
                error!("Connect host is not socket addr: {:?}", req.uri());
                let mut resp = Response::default();
                *resp.status_mut() = http::StatusCode::BAD_REQUEST;

                return Ok(resp);
            }
        };

        let port = req.uri().port_u16().unwrap_or(80);

        let addr = match Address::try_from(format!("{}:{}", host, port)) {
            Ok(addr) => addr,
            Err(_error) => {
                error!("{_error}");
                let mut resp = Response::default();
                *resp.status_mut() = http::StatusCode::BAD_REQUEST;

                return Ok(resp);
            }
        };

        debug!("Connect {:?}", addr);

        let mut outbound = match conn.connect(addr.clone()).await {
            Ok(conn) => conn,
            Err(_error) => {
                let mut resp = Response::default();
                *resp.status_mut() = http::StatusCode::BAD_REQUEST;

                return Ok(resp);
            }
        };

        if Method::CONNECT == req.method() {
            tokio::spawn(async move {
                match hyper::upgrade::on(req).await {
                    Ok(upgraded) => {
                        let mut stream = TokioIo::new(upgraded);

                        match copy_bidirectional(&mut stream, &mut outbound).await {
                            Ok(_copy) => {
                                info!("Connect {}, Send: {}, Recv: {}", addr, _copy.0, _copy.1);
                            }

                            Err(_error) => {
                                error!("{_error}")
                            }
                        }
                    }
                    Err(_error) => {
                        error!("Upgrade error: {}", _error);
                    }
                }
            });
        } else {
            let io = TokioIo::new(outbound);

            let (mut sender, conn) = ClientBuilder::new()
                .preserve_header_case(true)
                .title_case_headers(true)
                .handshake(io)
                .await?;

            tokio::spawn(async move {
                info!("Connect {}", addr);
                if let Err(err) = conn.await {
                    error!("Connection failed: {:?}", err);
                }
            });

            let resp = sender.send_request(req).await?;

            return Ok(resp.map(|b| b.boxed()));
        }

        Ok(Response::default())
    }
}

// ===== Tokio IO =====
pub struct TokioIo<T: Unpin> {
    inner: T,
}

mod tokiort {
    use super::TokioIo;

    use std::io::{Error, IoSlice};
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use hyper::rt::{Read, ReadBuf as HyperReadBuf, ReadBufCursor, Write};
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

    impl<T: Unpin> TokioIo<T> {
        pub fn new(inner: T) -> Self {
            Self { inner }
        }
    }

    impl<T> Read for TokioIo<T>
    where
        T: AsyncRead + Unpin,
    {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            mut buf: ReadBufCursor<'_>,
        ) -> Poll<Result<(), Error>> {
            let n = unsafe {
                let mut tbuf = ReadBuf::uninit(buf.as_mut());
                match AsyncRead::poll_read(Pin::new(&mut self.inner), cx, &mut tbuf) {
                    Poll::Ready(Ok(())) => tbuf.filled().len(),
                    other => return other,
                }
            };

            unsafe {
                buf.advance(n);
            }
            Poll::Ready(Ok(()))
        }
    }

    impl<T> Write for TokioIo<T>
    where
        T: AsyncWrite + Unpin,
    {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<Result<usize, Error>> {
            AsyncWrite::poll_write(Pin::new(&mut self.inner), cx, buf)
        }

        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
            AsyncWrite::poll_flush(Pin::new(&mut self.inner), cx)
        }

        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), Error>> {
            AsyncWrite::poll_shutdown(Pin::new(&mut self.inner), cx)
        }

        fn is_write_vectored(&self) -> bool {
            AsyncWrite::is_write_vectored(&self.inner)
        }

        fn poll_write_vectored(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            bufs: &[IoSlice<'_>],
        ) -> Poll<Result<usize, Error>> {
            AsyncWrite::poll_write_vectored(Pin::new(&mut self.inner), cx, bufs)
        }
    }

    impl<T> AsyncRead for TokioIo<T>
    where
        T: Read + Unpin,
    {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            tbuf: &mut ReadBuf<'_>,
        ) -> Poll<Result<(), Error>> {
            let filled = tbuf.filled().len();
            let sub_filled = unsafe {
                let mut buf = HyperReadBuf::uninit(tbuf.unfilled_mut());

                match Read::poll_read(Pin::new(&mut self.inner), cx, buf.unfilled()) {
                    Poll::Ready(Ok(())) => buf.filled().len(),
                    other => return other,
                }
            };

            let n_filled = filled + sub_filled;

            let n_init = sub_filled;
            unsafe {
                tbuf.assume_init(n_init);
                tbuf.set_filled(n_filled);
            }

            Poll::Ready(Ok(()))
        }
    }

    impl<T> AsyncWrite for TokioIo<T>
    where
        T: Write + Unpin,
    {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<Result<usize, Error>> {
            Write::poll_write(Pin::new(&mut self.inner), cx, buf)
        }

        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
            Write::poll_flush(Pin::new(&mut self.inner), cx)
        }

        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), Error>> {
            Write::poll_shutdown(Pin::new(&mut self.inner), cx)
        }

        fn is_write_vectored(&self) -> bool {
            Write::is_write_vectored(&self.inner)
        }

        fn poll_write_vectored(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            bufs: &[IoSlice<'_>],
        ) -> Poll<Result<usize, Error>> {
            Write::poll_write_vectored(Pin::new(&mut self.inner), cx, bufs)
        }
    }
}
