use std::error::Error;

use ombrac_protocol::Provider;
use tokio::sync::mpsc::Receiver;

use super::{connection, stream, Config};

pub mod impl_s2n_quic {
    use std::path::Path;

    use s2n_quic::provider::congestion_controller;
    use s2n_quic::provider::limits;
    use s2n_quic::stream::BidirectionalStream;
    use s2n_quic::Server;

    use connection::impl_s2n_quic::connection;
    use stream::impl_s2n_quic::stream;

    use super::*;

    pub struct NoiseQuic {
        stream: Receiver<BidirectionalStream>,
    }

    impl Provider<BidirectionalStream> for NoiseQuic {
        async fn fetch(&mut self) -> Option<BidirectionalStream> {
            self.stream.recv().await
        }
    }

    impl NoiseQuic {
        pub async fn with(config: Config) -> Result<Self, Box<dyn Error>> {
            let limits = {
                let mut limits = limits::Limits::new();

                if let Some(value) = config.bidirectional_local_data_window {
                    limits = limits.with_bidirectional_local_data_window(value)?;
                }

                if let Some(value) = config.bidirectional_remote_data_window {
                    limits = limits.with_bidirectional_remote_data_window(value)?;
                }

                if let Some(value) = config.max_open_bidirectional_streams {
                    limits = limits.with_max_open_local_bidirectional_streams(value)?;
                }

                if let Some(value) = config.max_open_bidirectional_streams {
                    limits = limits.with_max_open_remote_bidirectional_streams(value)?;
                }

                if let Some(value) = config.max_handshake_duration {
                    limits = limits.with_max_handshake_duration(value)?;
                }

                if let Some(value) = config.max_keep_alive_period {
                    limits = limits.with_max_keep_alive_period(value)?;
                }

                if let Some(value) = config.max_idle_timeout {
                    limits = limits.with_max_idle_timeout(value)?;
                }

                limits
            };

            let controller = {
                let mut controller = congestion_controller::bbr::Builder::default();

                if let Some(value) = config.initial_congestion_window {
                    controller = controller.with_initial_congestion_window(value);
                }

                controller.build()
            };

            let server = Server::builder()
                .with_io(config.listen)?
                .with_limits(limits)?
                .with_congestion_controller(controller)?
                .with_tls((Path::new(&config.tls_cert), Path::new(&config.tls_key)))?
                .start()?;

            let connection = connection(server);

            let stream = stream(connection).await;

            Ok(Self { stream })
        }
    }
}
