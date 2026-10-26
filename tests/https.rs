// HTTPS is an actual TLS connection: never a plaintext fallback or an
// insecure certificate-verification bypass. All tests use loopback sockets
// and synthetic test-only certificates, with no external Internet access.
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use browser_engine::net::{
    fetch::FetchRequest, DefaultLoader, HttpConfig, HttpLoader, ResourceLoader, Url,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};

const CA: &[u8] = include_bytes!("tls/ca.der");
const LEAF: &[u8] = include_bytes!("tls/leaf.der");
const KEY: &[u8] = include_bytes!("tls/leaf_key.der");

fn tls_server(response: &'static [u8], close_notify: bool) -> (u16, JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    let server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(LEAF.to_vec())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(KEY.to_vec())),
        )
        .expect("synthetic TLS server cert");
    let handle = thread::spawn(move || {
        let (tcp, _) = listener.accept().expect("one loopback client");
        tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        tcp.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        let connection = ServerConnection::new(Arc::new(server_config)).unwrap();
        let mut tls = StreamOwned::new(connection, tcp);
        let mut request = [0; 8192];
        let n = match tls.read(&mut request) {
            Ok(n) => n,
            Err(_) => return String::new(), // Expected for invalid client trust/name.
        };
        let received = String::from_utf8_lossy(&request[..n]).to_string();
        let _ = tls.write_all(response);
        if close_notify {
            tls.conn.send_close_notify();
        }
        let _ = tls.flush();
        received
    });
    (port, handle)
}

fn verified_loader() -> HttpLoader {
    let mut config = HttpConfig::default();
    config.extra_tls_roots.push(CA.to_vec());
    HttpLoader { config }
}

#[test]
fn verified_https_get_loads_a_real_tls_page_without_close_notify() {
    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Type: text/plain\r\n\r\nhello";
    let (port, handle) = tls_server(response, false);
    let url = Url::parse(&format!("https://localhost:{port}/hello?x=1")).unwrap();
    let resource = verified_loader().load(&url).expect("verified TLS page loads");
    assert_eq!(resource.text(), "hello");
    let request = handle.join().unwrap();
    assert!(request.starts_with("GET /hello?x=1 HTTP/1.1\r\n"), "{request}");
    assert!(request.contains(&format!("Host: localhost:{port}\r\n")), "{request}");
}

#[test]
fn clean_tls_close_delimits_body_without_length_headers() {
    let response = b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<p>hi</p>";
    let (port, handle) = tls_server(response, true);
    let url = Url::parse(&format!("https://localhost:{port}/close-delimited")).unwrap();
    let resource = verified_loader()
        .load(&url)
        .expect("TLS close_notify must terminate an unframed HTTP response");
    assert_eq!(resource.text(), "<p>hi</p>");
    handle.join().unwrap();
}

#[test]
fn unclean_tls_close_must_not_delimit_body_without_length_headers() {
    let response = b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<p>hi</p>";
    let (port, handle) = tls_server(response, false);
    let url = Url::parse(&format!("https://localhost:{port}/unclean-close")).unwrap();
    let error = verified_loader()
        .fetch(&FetchRequest::get(url))
        .expect_err("a missing close_notify must not certify a complete response");
    assert!(error.to_string().starts_with("TypeError"), "{error}");
    handle.join().unwrap();
}

#[test]
fn public_roots_reject_a_private_test_ca() {
    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
    let (port, handle) = tls_server(response, true);
    let url = Url::parse(&format!("https://localhost:{port}/")).unwrap();
    let error = HttpLoader::default()
        .fetch(&FetchRequest::get(url))
        .expect_err("untrusted test CA must never be accepted");
    assert!(error.to_string().starts_with("TypeError"), "{error}");
    assert!(!error.to_string().contains("unsupported scheme"), "{error}");
    handle.join().unwrap();
}

#[test]
fn extra_root_does_not_bypass_hostname_verification() {
    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
    let (port, handle) = tls_server(response, true);
    let url = Url::parse(&format!("https://127.0.0.1:{port}/")).unwrap();
    let error = verified_loader()
        .fetch(&FetchRequest::get(url))
        .expect_err("CA is trusted, but leaf only covers localhost, not IP");
    assert!(error.to_string().starts_with("TypeError"), "{error}");
    handle.join().unwrap();
}

#[test]
fn a_truncated_tls_response_does_not_become_a_success() {
    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nshort";
    let (port, handle) = tls_server(response, false);
    let url = Url::parse(&format!("https://localhost:{port}/truncated")).unwrap();
    let error = verified_loader()
        .fetch(&FetchRequest::get(url))
        .expect_err("truncated TLS responses must fail");
    assert!(error.to_string().starts_with("TypeError"), "{error}");
    handle.join().unwrap();
}

#[test]
fn fetch_once_uses_verified_tls_without_following_redirects() {
    let response = b"HTTP/1.1 302 Found\r\nLocation: /redirected\r\nContent-Length: 0\r\n\r\n";
    let (port, handle) = tls_server(response, false);
    let url = Url::parse(&format!("https://localhost:{port}/first")).unwrap();
    let fetched = verified_loader()
        .fetch_once(&FetchRequest::get(url.clone()))
        .expect("TLS send_once");
    assert_eq!(fetched.status, 302);
    assert_eq!(fetched.url, url);
    assert!(!fetched.redirected);
    handle.join().unwrap();
}

#[test]
fn default_loader_dispatches_https_not_just_http_loader() {
    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nyes";
    let (port, handle) = tls_server(response, true);
    let mut config = HttpConfig::default();
    config.extra_tls_roots.push(CA.to_vec());
    let loader = DefaultLoader::new().with_http_config(config);
    let url = Url::parse(&format!("https://localhost:{port}/")).unwrap();
    assert_eq!(loader.load(&url).unwrap().text(), "yes");
    handle.join().unwrap();
}
