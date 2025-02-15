#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, time::Duration};

    use curl::easy::Easy;
    use tests_support::cert::CertificateGenerator;
    use tests_support::net::http::MockServer;
    use tests_support::net::*;

    use tests_support::binary::{ClientBuilder, ServerBuilder};

    fn curl_proxy_connection(proxy_type: &str, socks_addr: SocketAddr, server_addr: SocketAddr) {
        let mut handle = Easy::new();
        handle.get(true).unwrap();
        handle.url(&format!("http://{}/", server_addr)).unwrap();
        handle
            .proxy(&format!("{}://{}", proxy_type, socks_addr))
            .unwrap();
        handle.timeout(Duration::from_secs(3)).unwrap();

        let mut resp = Vec::new();
        {
            let mut transfer = handle.transfer();
            transfer
                .write_function(|data| {
                    resp.extend_from_slice(data);
                    Ok(data.len())
                })
                .unwrap();

            transfer.perform().unwrap();
        }

        assert_eq!(handle.response_code().unwrap(), 200);
        assert_eq!(resp.as_slice(), b"Hello, World!");
    }

    // Integration test for Ombrac SOCKS + QUIC
    #[test]
    fn test_ombrac_socks_quic() {
        let server_addr = find_available_udp_addr("127.0.0.1".parse().unwrap());
        let client_socks_addr = find_available_tcp_addr("127.0.0.1".parse().unwrap());
        let mock_http_server_addr = find_available_tcp_addr("127.0.0.1".parse().unwrap());
        let (cert_path, key_path) = CertificateGenerator::generate();

        let _server = ServerBuilder::default()
            .secret("secret".to_string())
            .listen(server_addr.to_string())
            .tls_cert(cert_path.display().to_string())
            .tls_key(key_path.display().to_string())
            .build();

        let _client = ClientBuilder::default()
            .secret("secret".to_string())
            .socks(client_socks_addr.to_string())
            .server(server_addr.to_string())
            .tls_cert(cert_path.display().to_string())
            .build();

        assert!(
            wait_for_tcp_connect(&client_socks_addr, 10, 3000),
            "ombrac-client did not start listening on {}",
            client_socks_addr
        );

        let _mock_server = MockServer::start(mock_http_server_addr, |_req| {
            b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\nHello, World!".to_vec()
        });

        curl_proxy_connection("socks5", client_socks_addr, mock_http_server_addr);
        curl_proxy_connection("socks5h", client_socks_addr, mock_http_server_addr);
    }

    mod tls {
        use super::*;

        #[test]
        fn test_ombrac_socks_quic_tls_skip() {
            let server_addr = find_available_udp_addr("127.0.0.1".parse().unwrap());
            let client_socks_addr = find_available_tcp_addr("127.0.0.1".parse().unwrap());
            let mock_http_server_addr = find_available_tcp_addr("127.0.0.1".parse().unwrap());
            let (cert_path, key_path) = CertificateGenerator::generate();

            let _server = ServerBuilder::default()
                .secret("secret".to_string())
                .listen(server_addr.to_string())
                .tls_cert(cert_path.display().to_string())
                .tls_key(key_path.display().to_string())
                .build();

            let _client = ClientBuilder::default()
                .secret("secret".to_string())
                .socks(client_socks_addr.to_string())
                .server(server_addr.to_string())
                .tls_skip("true".to_string())
                .build();

            assert!(
                wait_for_tcp_connect(&client_socks_addr, 10, 3000),
                "ombrac-client did not start listening on {}",
                client_socks_addr
            );

            let _mock_server = MockServer::start(mock_http_server_addr, |_req| {
                b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\nHello, World!".to_vec()
            });

            curl_proxy_connection("socks5", client_socks_addr, mock_http_server_addr);
            curl_proxy_connection("socks5h", client_socks_addr, mock_http_server_addr);
        }

        #[test]
        fn test_ombrac_socks_quic_tls_self_signed() {
            let server_addr = find_available_udp_addr("127.0.0.1".parse().unwrap());
            let client_socks_addr = find_available_tcp_addr("127.0.0.1".parse().unwrap());
            let mock_http_server_addr = find_available_tcp_addr("127.0.0.1".parse().unwrap());

            let _server = ServerBuilder::default()
                .secret("secret".to_string())
                .listen(server_addr.to_string())
                .tls_skip("true".to_string())
                .build();

            let _client = ClientBuilder::default()
                .secret("secret".to_string())
                .socks(client_socks_addr.to_string())
                .server(server_addr.to_string())
                .tls_skip("true".to_string())
                .build();

            assert!(
                wait_for_tcp_connect(&client_socks_addr, 10, 3000),
                "ombrac-client did not start listening on {}",
                client_socks_addr
            );

            let _mock_server = MockServer::start(mock_http_server_addr, |_req| {
                b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\nHello, World!".to_vec()
            });

            curl_proxy_connection("socks5", client_socks_addr, mock_http_server_addr);
            curl_proxy_connection("socks5h", client_socks_addr, mock_http_server_addr);
        }
    }
}
