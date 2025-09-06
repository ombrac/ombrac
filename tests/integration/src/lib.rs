mod quic;

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, time::Duration};

    use curl::easy::Easy;
    use tests_support::cert::CertificateGenerator;
    use tests_support::net::http::MockServer;
    use tests_support::net::*;

    use tests_support::binary::{Client, Server};

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
    #[ntest::timeout(60000)]
    fn test_ombrac_socks_quic() {
        let server_addr = find_available_local_udp_addr();
        let client_socks_addr = find_available_local_tcp_addr();
        let mock_http_server_addr = find_available_local_tcp_addr();
        let (cert_path, key_path) = CertificateGenerator::generate();

        let _server = Server::default()
            .secret("secret".to_string())
            .listen(server_addr.to_string())
            .tls_cert(cert_path.display().to_string())
            .tls_key(key_path.display().to_string())
            .start();

        let _client = Client::default()
            .secret("secret".to_string())
            .socks(client_socks_addr.to_string())
            .server(server_addr.to_string())
            .ca_cert(cert_path.display().to_string())
            .start();

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
        #[ntest::timeout(60000)]
        fn test_ombrac_socks_quic_insecure() {
            let server_addr = find_available_local_udp_addr();
            let client_socks_addr = find_available_local_tcp_addr();
            let mock_http_server_addr = find_available_local_tcp_addr();

            let _server = Server::default()
                .secret("secret".to_string())
                .listen(server_addr.to_string())
                .tls_mode("insecure".to_string())
                .start();

            let _client = Client::default()
                .secret("secret".to_string())
                .socks(client_socks_addr.to_string())
                .server(server_addr.to_string())
                .tls_mode("insecure".to_string())
                .start();

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

    #[test]
    #[ntest::timeout(60000)]
    fn test_ombrac_socks_quic_mtls() {
        let (ca_cert_path, _) = CertificateGenerator::generate();
        let (server_cert_path, server_key_path) = CertificateGenerator::generate();
        let (client_cert_path, client_key_path) = CertificateGenerator::generate();

        let server_addr = find_available_local_udp_addr();
        let client_socks_addr = find_available_local_tcp_addr();
        let mock_http_server_addr = find_available_local_tcp_addr();

        let _server = Server::default()
            .secret("secret".to_string())
            .listen(server_addr.to_string())
            .tls_cert(server_cert_path.display().to_string())
            .tls_key(server_key_path.display().to_string())
            .tls_mode("m-tls".to_string())
            .tls_ca(ca_cert_path.display().to_string())
            .start();

        let _client = Client::default()
            .secret("secret".to_string())
            .socks(client_socks_addr.to_string())
            .server(server_addr.to_string())
            .tls_mode("m-tls".to_string())
            .client_cert(client_cert_path.display().to_string())
            .client_key(client_key_path.display().to_string())
            .ca_cert(ca_cert_path.display().to_string())
            .start();

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
