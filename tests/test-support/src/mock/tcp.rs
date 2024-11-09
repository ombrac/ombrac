use std::error::Error;
use std::future::Future;

use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};

pub async fn server<T, F, Fu>(addr: T, func: F) -> Result<(), Box<dyn Error>>
where
    T: ToSocketAddrs,
    F: Fn(TcpStream) -> Fu + Clone + Send + 'static,
    Fu: Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind(addr).await?;

    while let Ok((socket, _)) = listener.accept().await {
        let func = func.clone();
        tokio::spawn(async move {
            func(socket).await;
        });
    }

    Err("Failed to accept".into())
}
