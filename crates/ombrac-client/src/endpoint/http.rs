use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, combinators::BoxBody};
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

use ombrac::protocol::Address;
use ombrac_macros::{error, info};
use ombrac_transport::quic::Connection as QuicConnection;
use ombrac_transport::quic::client::Client as QuicClient;

use crate::client::Client;
use crate::connection::BufferedStream;

type HttpResult = Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error>;
type HyperClientBuilder = hyper::client::conn::http1::Builder;
type HyperServerBuilder = hyper::server::conn::http1::Builder;

pub struct Server {
    client: Arc<Client<QuicClient, QuicConnection>>,
}

impl Server {
    pub fn new(client: Arc<Client<QuicClient, QuicConnection>>) -> Self {
        Self { client }
    }

    pub async fn accept_loop(
        &self,
        listener: TcpListener,
        shutdown_signal: impl Future<Output = ()>,
    ) -> io::Result<()> {
        tokio::pin!(shutdown_signal);

        loop {
            tokio::select! {
                biased;
                _ = &mut shutdown_signal => {
                    return Ok(());
                },
                result = listener.accept() => {
                    let (stream, remote_addr) = match result {
                        Ok(res) => res,
                        Err(e) => {
                            error!(error = %e, "failed to accept connection");
                            continue;
                        }
                    };

                    let client = self.client.clone();
                    tokio::spawn(async move {
                        let io = TokioIo::new(stream);
                        let service = hyper::service::service_fn(move |req| {
                            Self::proxy_handler(req, client.clone(), remote_addr)
                        });

                        if let Err(e) = HyperServerBuilder::new()
                            .preserve_header_case(true)
                            .title_case_headers(true)
                            .serve_connection(io, service)
                            .with_upgrades()
                            .await
                            && !is_connection_closed_error(&e) {
                                error!(
                                    src_addr = %remote_addr,
                                    error = %e,
                                    "failed to serve connection"
                                );
                            }
                    });
                }
            }
        }
    }

    async fn proxy_handler(
        req: Request<hyper::body::Incoming>,
        client: Arc<Client<QuicClient, QuicConnection>>,
        remote_addr: SocketAddr,
    ) -> HttpResult {
        let target_addr = match Self::extract_target_address(&req) {
            Ok(addr) => addr,
            Err(response) => return Ok(*response),
        };

        let outbound_conn = match client.open_bidirectional(target_addr.clone()).await {
            Ok(conn) => conn,
            Err(e) => {
                error!(
                    dst_addr = %target_addr,
                    error = %e,
                    "failed to open outbound connection"
                );
                return Ok(Self::create_error_response(StatusCode::SERVICE_UNAVAILABLE));
            }
        };

        if req.method() == Method::CONNECT {
            Self::handle_connect(req, outbound_conn, remote_addr, target_addr).await
        } else {
            Self::handle_http(req, outbound_conn, remote_addr, target_addr).await
        }
    }

    async fn handle_connect(
        req: Request<hyper::body::Incoming>,
        mut dest_stream: BufferedStream<<QuicConnection as ombrac_transport::Connection>::Stream>,
        remote_addr: SocketAddr,
        target_addr: Address,
    ) -> HttpResult {
        tokio::spawn(async move {
            match hyper::upgrade::on(req).await {
                Ok(upgraded) => {
                    let mut upgraded_io = TokioIo::new(upgraded);
                    match ombrac_transport::io::copy_bidirectional(
                        &mut upgraded_io,
                        &mut dest_stream,
                    )
                    .await
                    {
                        Ok(stats) => {
                            info!(
                                src_addr = %remote_addr,
                                dst_addr = %target_addr,
                                send = stats.a_to_b_bytes,
                                recv = stats.b_to_a_bytes,
                                "tcp connect"
                            );
                        }
                        Err((err, stats)) => {
                            error!(
                                src_addr = %remote_addr,
                                dst_addr = %target_addr,
                                send = stats.a_to_b_bytes,
                                recv = stats.b_to_a_bytes,
                                error = %err,
                                "tcp connect"
                            );
                        }
                    }
                }
                Err(e) => {
                    error!(
                        src_addr = %remote_addr,
                        error = %e,
                        "upgrade error"
                    );
                }
            }
        });

        Ok(Response::new(empty_body()))
    }

    async fn handle_http(
        req: Request<hyper::body::Incoming>,
        outbound_conn: BufferedStream<<QuicConnection as ombrac_transport::Connection>::Stream>,
        remote_addr: SocketAddr,
        target_addr: Address,
    ) -> HttpResult {
        let io = TokioIo::new(outbound_conn);

        let (mut sender, conn) = HyperClientBuilder::new()
            .preserve_header_case(true)
            .title_case_headers(true)
            .handshake(io)
            .await?;

        tokio::spawn(async move {
            if let Err(err) = conn.await {
                error!(
                    src_addr = %remote_addr,
                    dst_addr = %target_addr,
                    error = ?err,
                    "connection failed"
                );
            }
        });

        let resp = sender.send_request(req).await?;
        Ok(resp.map(|b| b.boxed()))
    }

    fn extract_target_address(
        req: &Request<hyper::body::Incoming>,
    ) -> Result<Address, Box<Response<BoxBody<Bytes, hyper::Error>>>> {
        let host = req.uri().host().ok_or_else(|| {
            error!("request uri does not contain a host");
            Self::create_error_response(StatusCode::BAD_REQUEST)
        })?;

        let port = req
            .uri()
            .port_u16()
            .unwrap_or(if req.uri().scheme_str() == Some("https") {
                443
            } else {
                80
            });
        let addr_str = format!("{}:{}", host, port);

        Ok(Address::try_from(addr_str).map_err(|e| {
            error!(
                error = %e,
                "invalid target address format"
            );
            Self::create_error_response(StatusCode::BAD_REQUEST)
        })?)
    }

    fn create_error_response(status: StatusCode) -> Response<BoxBody<Bytes, hyper::Error>> {
        let mut resp = Response::new(empty_body());
        *resp.status_mut() = status;
        resp
    }
}

fn empty_body() -> BoxBody<Bytes, hyper::Error> {
    http_body_util::Empty::new()
        .map_err(|never| match never {})
        .boxed()
}

fn is_connection_closed_error(e: &hyper::Error) -> bool {
    e.to_string().contains("connection closed")
}
