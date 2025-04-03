use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::thread;
use std::time::Duration;

pub fn find_available_tcp_addr(ip: IpAddr) -> SocketAddr {
    let listener = TcpListener::bind(format!("{}:0", ip)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    SocketAddr::new(ip, port)
}

pub fn find_available_local_tcp_addr() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    addr
}

pub fn find_available_udp_addr(ip: IpAddr) -> SocketAddr {
    let listener = UdpSocket::bind(format!("{}:0", ip)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    SocketAddr::new(ip, port)
}

pub fn find_available_local_udp_addr() -> SocketAddr {
    let listener = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    addr
}

pub fn wait_for_tcp_connect(addr: &SocketAddr, retries: u32, wait_millis: u64) -> bool {
    for _ in 0..retries {
        if TcpStream::connect(addr).is_ok() {
            return true;
        }
        thread::sleep(Duration::from_millis(wait_millis));
    }

    false
}

pub mod http {
    use std::io::{Read, Write};
    use std::net::ToSocketAddrs;
    use std::thread::JoinHandle;

    use super::*;

    pub struct MockServer {
        pub handle: JoinHandle<()>,
    }

    impl MockServer {
        pub fn start<A, F>(addr: A, handler: F) -> Self
        where
            A: ToSocketAddrs,
            F: Fn(&str) -> Vec<u8> + Send + 'static,
        {
            let listener = TcpListener::bind(addr).unwrap();

            let handle = thread::spawn(move || {
                let mut buf = [0; 1024];
                while let Ok((mut conn, _)) = listener.accept() {
                    let n = conn.read(&mut buf).unwrap();
                    let req = String::from_utf8_lossy(&buf[..n]);

                    let response = handler(&req);
                    conn.write_all(&response).unwrap();
                }
            });

            Self { handle }
        }
    }
}

pub mod tcp {
    use std::{collections::HashMap, io, net::SocketAddr, sync::Arc};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::Mutex;

    pub struct ResponseTcpServer {
        responses: Arc<Mutex<HashMap<Vec<u8>, Vec<u8>>>>,
        addr: SocketAddr,
        delay_ms: Option<u64>,
    }

    impl ResponseTcpServer {
        pub async fn new() -> io::Result<Self> {
            Ok(Self {
                responses: Arc::new(Mutex::new(HashMap::new())),
                addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                delay_ms: None,
            })
        }

        pub async fn set_response(&self, input: Vec<u8>, response: Vec<u8>) {
            let mut responses = self.responses.lock().await;
            responses.insert(input, response);
        }

        pub fn with_delay(mut self, delay_ms: u64) -> Self {
            self.delay_ms = Some(delay_ms);
            self
        }

        pub fn addr(&self) -> SocketAddr {
            self.addr
        }

        pub async fn start(self) -> io::Result<ServerHandle> {
            let listener = TcpListener::bind(self.addr).await?;
            let actual_addr = listener.local_addr()?;

            let responses = self.responses.clone();
            let delay_ms = self.delay_ms;

            let handle = tokio::spawn(async move {
                loop {
                    match listener.accept().await {
                        Ok((stream, _)) => {
                            let responses = responses.clone();
                            tokio::spawn(async move {
                                if let Err(e) =
                                    Self::handle_connection(stream, responses, delay_ms).await
                                {
                                    eprintln!("Error handling connection: {}", e);
                                }
                            });
                        }
                        Err(e) => {
                            eprintln!("Error accepting connection: {}", e);
                            break;
                        }
                    }
                }
            });

            Ok(ServerHandle {
                handle,
                addr: actual_addr,
            })
        }

        async fn handle_connection(
            mut stream: TcpStream,
            responses: Arc<Mutex<HashMap<Vec<u8>, Vec<u8>>>>,
            delay_ms: Option<u64>,
        ) -> io::Result<()> {
            let mut buffer = vec![0; 1024];

            loop {
                let n = stream.read(&mut buffer).await?;
                if n == 0 {
                    break;
                }

                let input = buffer[..n].to_vec();
                let responses = responses.lock().await;

                if let Some(response) = responses.get(&input) {
                    if let Some(delay) = delay_ms {
                        tokio::time::sleep(tokio::time::Duration::from_millis(delay)).await;
                    }

                    stream.write_all(response).await?;
                }
            }

            Ok(())
        }
    }

    pub struct EchoTcpServer {
        addr: SocketAddr,
    }

    impl Default for EchoTcpServer {
        fn default() -> Self {
            Self {
                addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            }
        }
    }

    impl EchoTcpServer {
        pub fn new() -> Self {
            Self {
                addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            }
        }

        pub fn addr(&self) -> SocketAddr {
            self.addr
        }

        pub async fn start(self) -> io::Result<ServerHandle> {
            let listener = TcpListener::bind(self.addr).await?;
            let actual_addr = listener.local_addr()?;

            let handle = tokio::spawn(async move {
                loop {
                    match listener.accept().await {
                        Ok((stream, _)) => {
                            tokio::spawn(async move {
                                if let Err(e) = Self::handle_connection(stream).await {
                                    eprintln!("Error handling connection: {}", e);
                                }
                            });
                        }
                        Err(e) => {
                            eprintln!("Error accepting connection: {}", e);
                            break;
                        }
                    }
                }
            });

            Ok(ServerHandle {
                handle,
                addr: actual_addr,
            })
        }

        async fn handle_connection(mut stream: TcpStream) -> io::Result<()> {
            let mut buffer = vec![0; 1024];

            loop {
                let n = stream.read(&mut buffer).await?;
                if n == 0 {
                    break;
                }

                stream.write_all(&buffer[..n]).await?;
            }

            Ok(())
        }
    }

    pub struct ServerHandle {
        handle: tokio::task::JoinHandle<()>,
        addr: SocketAddr,
    }

    impl ServerHandle {
        pub fn addr(&self) -> SocketAddr {
            self.addr
        }

        pub fn abort(&self) {
            self.handle.abort();
        }
    }

    #[cfg(test)]
    mod tests_response {
        use super::ResponseTcpServer;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpStream;

        #[tokio::test]
        async fn test_response_tcp_server() {
            let server = ResponseTcpServer::new().await.unwrap();

            let input = b"hello";
            let response = b"world";
            server.set_response(input.to_vec(), response.to_vec()).await;

            let handle = server.start().await.unwrap();
            let server_addr = handle.addr();

            let mut stream = TcpStream::connect(server_addr).await.unwrap();

            stream.write_all(input).await.unwrap();

            let mut buf = [0u8; 1024];
            let n = stream.read(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], response);

            handle.abort();
        }
    }

    #[cfg(test)]
    mod tests_echo {
        use super::EchoTcpServer;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpStream;

        #[tokio::test]
        async fn test_echo_tcp_server() {
            let server = EchoTcpServer::new();
            let handle = server.start().await.unwrap();
            let server_addr = handle.addr();

            let mut stream = TcpStream::connect(server_addr).await.unwrap();

            let input = b"hello";
            stream.write_all(input).await.unwrap();

            let mut buf = [0u8; 1024];
            let n = stream.read(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], input);

            handle.abort();
        }
    }
}

pub mod udp {
    use std::{collections::HashMap, io, net::SocketAddr, sync::Arc, time::Duration};

    use tokio::{net::UdpSocket, sync::Mutex};

    pub struct ResponseUdpServer {
        responses: Arc<Mutex<HashMap<Vec<u8>, Vec<u8>>>>,
        addr: SocketAddr,
        delay_ms: Option<u64>,
        buffer_size: usize,
    }

    impl Default for ResponseUdpServer {
        fn default() -> Self {
            Self {
                responses: Arc::new(Mutex::new(HashMap::new())),
                addr: SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], 0)),
                delay_ms: None,
                buffer_size: 4096,
            }
        }
    }

    impl ResponseUdpServer {
        pub async fn set_response(&self, input: Vec<u8>, response: Vec<u8>) {
            let mut responses = self.responses.lock().await;
            responses.insert(input, response);
        }

        pub fn with_delay(mut self, delay_ms: u64) -> Self {
            self.delay_ms = Some(delay_ms);
            self
        }

        pub fn with_buffer_size(mut self, buffer_size: usize) -> Self {
            self.buffer_size = buffer_size;
            self
        }

        pub fn addr(&self) -> SocketAddr {
            self.addr
        }

        pub async fn start(self) -> io::Result<UdpServerHandle> {
            let socket = Arc::new(UdpSocket::bind(self.addr).await?);
            let actual_addr = socket.local_addr()?;

            let responses = self.responses.clone();
            let delay_ms = self.delay_ms;
            let buffer_size = self.buffer_size;

            let socket_clone = socket.clone();
            let handle = tokio::spawn(async move {
                let mut buf = vec![0u8; buffer_size];

                loop {
                    match socket_clone.recv_from(&mut buf).await {
                        Ok((size, addr)) => {
                            let input = buf[..size].to_vec();
                            let socket = socket_clone.clone();
                            let responses = responses.clone();
                            let delay = delay_ms;

                            tokio::spawn(async move {
                                let responses_lock = responses.lock().await;
                                if let Some(response) = responses_lock.get(&input) {
                                    if let Some(delay_ms) = delay {
                                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                                    }

                                    if let Err(e) = socket.send_to(response, addr).await {
                                        eprintln!("Error sending UDP response: {}", e);
                                    }
                                }
                            });
                        }
                        Err(e) => {
                            eprintln!("Error receiving UDP packet: {}", e);
                            break;
                        }
                    }
                }
            });

            Ok(UdpServerHandle {
                handle,
                socket,
                addr: actual_addr,
            })
        }
    }

    pub struct EchoUdpServer {
        addr: SocketAddr,
        buffer_size: usize,
    }

    impl Default for EchoUdpServer {
        fn default() -> Self {
            Self {
                addr: SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], 0)),
                buffer_size: 4096,
            }
        }
    }

    impl EchoUdpServer {
        pub fn with_buffer_size(mut self, buffer_size: usize) -> Self {
            self.buffer_size = buffer_size;
            self
        }

        pub fn addr(&self) -> SocketAddr {
            self.addr
        }

        pub async fn start(self) -> io::Result<UdpServerHandle> {
            let socket = Arc::new(UdpSocket::bind(self.addr).await?);
            let actual_addr = socket.local_addr()?;

            let buffer_size = self.buffer_size;

            let socket_clone = socket.clone();
            let handle = tokio::spawn(async move {
                let mut buf = vec![0u8; buffer_size];

                loop {
                    match socket_clone.recv_from(&mut buf).await {
                        Ok((size, addr)) => {
                            let data = buf[..size].to_vec();
                            let socket = socket_clone.clone();

                            tokio::spawn(async move {
                                if let Err(e) = socket.send_to(&data, addr).await {
                                    eprintln!("Error sending UDP response: {}", e);
                                }
                            });
                        }
                        Err(e) => {
                            eprintln!("Error receiving UDP packet: {}", e);
                            break;
                        }
                    }
                }
            });

            Ok(UdpServerHandle {
                handle,
                socket,
                addr: actual_addr,
            })
        }
    }

    pub struct UdpServerHandle {
        handle: tokio::task::JoinHandle<()>,
        socket: Arc<UdpSocket>,
        addr: SocketAddr,
    }

    impl UdpServerHandle {
        pub fn addr(&self) -> SocketAddr {
            self.addr
        }

        pub fn abort(&self) {
            self.handle.abort();
        }

        pub async fn send_to(&self, data: &[u8], addr: SocketAddr) -> std::io::Result<usize> {
            self.socket.send_to(data, addr).await
        }
    }

    #[cfg(test)]
    mod tests_response {
        use super::ResponseUdpServer;
        use tokio::net::UdpSocket;

        #[tokio::test]
        async fn test_response_udp_server() {
            let server = ResponseUdpServer::default();
            let input = b"hello";
            let response = b"world";
            server.set_response(input.to_vec(), response.to_vec()).await;

            let handle = server.start().await.unwrap();
            let server_addr = handle.addr();

            let client_socket = UdpSocket::bind("[::1]:0").await.unwrap();
            client_socket.send_to(input, server_addr).await.unwrap();

            let mut buf = [0u8; 1024];
            let (size, _) = client_socket.recv_from(&mut buf).await.unwrap();
            assert_eq!(&buf[..size], response);

            handle.abort();
        }
    }

    #[cfg(test)]
    mod tests_echo {
        use super::EchoUdpServer;
        use tokio::net::UdpSocket;

        #[tokio::test]
        async fn test_echo_udp_server() {
            let server = EchoUdpServer::default();
            let handle = server.start().await.unwrap();
            let server_addr = handle.addr();

            let client_socket = UdpSocket::bind("[::1]:0").await.unwrap();
            let input = b"hello";
            client_socket.send_to(input, server_addr).await.unwrap();

            let mut buf = [0u8; 1024];
            let (size, _) = client_socket.recv_from(&mut buf).await.unwrap();
            assert_eq!(&buf[..size], input);

            handle.abort();
        }
    }
}
