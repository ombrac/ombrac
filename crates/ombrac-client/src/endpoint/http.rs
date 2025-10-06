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
use ombrac_transport::{Connection, Initiator};

use crate::client::Client;

type HttpResult = Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error>;
type HyperClientBuilder = hyper::client::conn::http1::Builder;
type HyperServerBuilder = hyper::server::conn::http1::Builder;

pub struct Server<T, C> {
    client: Arc<Client<T, C>>,
}

impl<T, C> Server<T, C>
where
    T: Initiator<Connection = C>,
    C: Connection,
{
    pub fn new(client: Arc<Client<T, C>>) -> Self {
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
                    info!("Shutdown signal received, exiting accept loop.");
                    return Ok(());
                },
                result = listener.accept() => {
                    let (stream, remote_addr) = match result {
                        Ok(res) => res,
                        Err(e) => {
                            error!("Failed to accept connection: {}", e);
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
                                error!("Failed to serve connection from {}: {}", remote_addr, e);
                            }
                    });
                }
            }
        }
    }

    async fn proxy_handler(
        req: Request<hyper::body::Incoming>,
        client: Arc<Client<T, C>>,
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
                    "Failed to open outbound connection to {}: {}",
                    target_addr, e
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
        mut dest_stream: C::Stream,
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
                            #[cfg(feature = "tracing")]
                            tracing::info!(
                                src_addr = remote_addr.to_string(),
                                dst_addr = target_addr.to_string(),
                                send = stats.a_to_b_bytes,
                                recv = stats.b_to_a_bytes,
                                status = "ok",
                                "Connect"
                            );
                        }
                        Err((err, stats)) => {
                            #[cfg(feature = "tracing")]
                            tracing::error!(
                                src_addr = remote_addr.to_string(),
                                dst_addr = target_addr.to_string(),
                                send = stats.a_to_b_bytes,
                                recv = stats.b_to_a_bytes,
                                status = "err",
                                error = %err,
                                "Connect"
                            );
                        }
                    }
                }
                Err(e) => {
                    error!("Upgrade error from {}: {}", remote_addr, e);
                }
            }
        });

        Ok(Response::new(empty_body()))
    }

    async fn handle_http(
        req: Request<hyper::body::Incoming>,
        outbound_conn: C::Stream,
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
                    "Connection to {} failed for {}: {:?}",
                    target_addr, remote_addr, err
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
            error!("Request URI does not contain a host");
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
            error!("Invalid target address format: {}", e);
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
