use std::sync::Arc;
use std::{io, net::SocketAddr};

use bytes::Bytes;
use http_body_util::{BodyExt, combinators::BoxBody};
use hyper::{Method, Request, Response};
use hyper_util::rt::TokioIo;
use ombrac::prelude::{Address, Client, Secret};
use ombrac_macros::{error, info};
use ombrac_transport::Initiator;
use tokio::net::TcpListener;

type ClientBuilder = hyper::client::conn::http1::Builder;
type ServerBuilder = hyper::server::conn::http1::Builder;

pub struct Server;

impl Server {
    pub async fn run<I>(
        listener: TcpListener,
        secret: Secret,
        ombrac_client: Arc<Client<I>>,
        shutdown_signal: impl Future<Output = ()>,
    ) -> io::Result<()>
    where
        I: Initiator,
    {
        let ombrac = Arc::clone(&ombrac_client);

        tokio::pin!(shutdown_signal);

        loop {
            tokio::select! {
                biased;
                _ = &mut shutdown_signal => return Ok(()),

                result = listener.accept() => {
                    let (stream, addr) = match result {
                        Ok(res) => res,
                        Err(_err) => {
                            error!("Failed to accept connection: {}", _err);
                            continue;
                        }
                    };

                    let ombrac = ombrac.clone();
                    tokio::spawn(async move {
                        let io = TokioIo::new(stream);
                        if let Err(_error) = ServerBuilder::new()
                            .preserve_header_case(true)
                            .title_case_headers(true)
                            .serve_connection(
                                io,
                                hyper::service::service_fn(|req| async {
                                    Self::tunnel(req, ombrac.clone(), secret, addr).await
                                }),
                            )
                            .with_upgrades()
                            .await
                        {
                            error!("Failed to serve connection: {}", _error);
                        }
                    });
                }
            }
        }
    }

    async fn tunnel<I>(
        req: Request<hyper::body::Incoming>,
        conn: Arc<Client<I>>,
        secret: Secret,
        _from_addr: SocketAddr,
    ) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error>
    where
        I: Initiator,
    {
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

        let target_addr = match Address::try_from(format!("{host}:{port}")) {
            Ok(addr) => addr,
            Err(_error) => {
                error!("{_error}");
                let mut resp = Response::default();
                *resp.status_mut() = http::StatusCode::BAD_REQUEST;

                return Ok(resp);
            }
        };

        let mut outbound = match conn.connect(target_addr.clone(), secret).await {
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
                                info!(
                                    "{} Connect {}, Send: {}, Recv: {}",
                                    _from_addr, target_addr, _copy.0, _copy.1
                                );
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
                info!("{_from_addr } Connect {target_addr}");
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
