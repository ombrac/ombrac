use crate::debug;

pub mod impl_s2n_quic {
    use s2n_quic::{Connection as NoiseConnection, Server as NoiseServer};
    use tokio::sync::mpsc;
    use tokio::sync::mpsc::Receiver;

    use super::*;

    pub fn connection(mut server: NoiseServer) -> Receiver<NoiseConnection> {
        let (sender, receiver) = mpsc::channel(1);

        tokio::spawn(async move {
            while let Some(connection) = server.accept().await {
                debug!(
                    "{:?} accpet connection {} from {:?}",
                    connection.server_name(),
                    connection.id(),
                    connection.remote_addr()
                );

                if sender.send(connection).await.is_err() {
                    break;
                }
            }
        });

        receiver
    }
}
