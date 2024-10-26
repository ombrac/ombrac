pub mod impl_s2n_quic {
    use s2n_quic::{Connection as NoiseConnection, Server as NoiseServer};
    use tokio::sync::mpsc;
    use tokio::sync::mpsc::Receiver;

    pub fn connection(mut server: NoiseServer) -> Receiver<NoiseConnection> {
        let (sender, receiver) = mpsc::channel(1);

        tokio::spawn(async move {
            while let Some(connection) = server.accept().await {
                if sender.send(connection).await.is_err() {
                    break;
                }
            }
        });

        receiver
    }
}
