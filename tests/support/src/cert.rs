use std::fs::File;
use std::io::Write;
use std::net::IpAddr;
use std::path::PathBuf;
use std::str::FromStr;

use rcgen::{Ia5String, SanType};

use crate::path::BinaryLocator;

pub struct CertificateGenerator;

impl CertificateGenerator {
    pub fn generate() -> (PathBuf, PathBuf) {
        let target_dir = BinaryLocator::locate("dummy")
            .parent()
            .unwrap()
            .to_path_buf();
        let cert_path = target_dir.join("cert.pem");
        let key_path = target_dir.join("key.pem");

        let mut cert_params = rcgen::CertificateParams::default();
        let name = Ia5String::from_str("localhost").unwrap();
        cert_params.subject_alt_names = vec![
            SanType::DnsName(name),
            SanType::IpAddress(IpAddr::from([127, 0, 0, 1])),
        ];

        let cert_key = rcgen::KeyPair::generate().unwrap();
        let cert = cert_params.self_signed(&cert_key).unwrap();

        File::create(&cert_path)
            .and_then(|mut f| f.write_all(cert.pem().as_bytes()))
            .expect("Failed to write cert file");

        File::create(&key_path)
            .and_then(|mut f| f.write_all(cert_key.serialize_pem().as_bytes()))
            .expect("Failed to write key file");

        (cert_path, key_path)
    }
}
