use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::thread;
use std::time::Duration;

pub fn find_available_tcp_addr(ip: IpAddr) -> SocketAddr {
    let listener = TcpListener::bind(format!("{}:0", ip)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    SocketAddr::new(ip, port)
}

pub fn find_available_udp_addr(ip: IpAddr) -> SocketAddr {
    let listener = UdpSocket::bind(format!("{}:0", ip)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    SocketAddr::new(ip, port)
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
