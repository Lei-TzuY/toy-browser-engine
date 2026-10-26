use std::io::{Read, Write};
use std::net::TcpListener;

use browser_engine::net::http::{send, send_once};
use browser_engine::net::{FetchRequest, HeaderMap, HttpConfig, Method, Url};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

#[test]
fn send_once_returns_redirect_without_following_location() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();

    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut request = [0u8; 2048];
        let count = stream.read(&mut request).expect("read");
        let request = String::from_utf8_lossy(&request[..count]).into_owned();
        stream
            .write_all(
                b"HTTP/1.1 302 Found\r\nLocation: /next\r\nX-Hop: first\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .expect("write");
        request
    });

    let start = url(&format!("http://127.0.0.1:{port}/start"));
    let response = send_once(&FetchRequest::get(start.clone()), &HttpConfig::default())
        .expect("single hop succeeds");

    assert_eq!(response.url, start);
    assert_eq!(response.status, 302);
    assert_eq!(response.headers.get("location").as_deref(), Some("/next"));
    assert_eq!(response.headers.get("x-hop").as_deref(), Some("first"));
    assert!(!response.redirected);
    assert!(response.body.is_empty());

    let request = server.join().expect("server joins");
    assert!(request.starts_with("GET /start HTTP/1.1\r\n"), "{request}");
}

#[test]
fn send_once_preserves_post_method_body_and_three_xx_response() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();

    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut bytes = Vec::new();
        loop {
            let mut chunk = [0u8; 2048];
            let count = stream.read(&mut chunk).expect("read");
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..count]);
            if let Some(split) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&bytes[..split]);
                let length = head
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                if bytes.len() >= split + 4 + length {
                    break;
                }
            }
        }
        stream
            .write_all(
                b"HTTP/1.1 307 Temporary Redirect\r\nLocation: /again\r\nContent-Length: 4\r\nConnection: close\r\n\r\nhop!",
            )
            .expect("write");
        String::from_utf8_lossy(&bytes).into_owned()
    });

    let mut headers = HeaderMap::new();
    headers.insert_raw("content-type", "text/plain");
    let request = FetchRequest::new(
        url(&format!("http://127.0.0.1:{port}/submit")),
        Method::Post,
        headers,
        Some(b"payload".to_vec()),
    );
    let response = send_once(&request, &HttpConfig::default()).expect("single hop succeeds");

    assert_eq!(response.status, 307);
    assert_eq!(response.headers.get("location").as_deref(), Some("/again"));
    assert_eq!(response.body, b"hop!");
    assert!(!response.redirected);

    let wire = server.join().expect("server joins");
    assert!(wire.starts_with("POST /submit HTTP/1.1\r\n"), "{wire}");
    assert!(wire.contains("content-type: text/plain\r\n"), "{wire}");
    assert!(wire.contains("\r\n\r\npayload"), "{wire}");
}

#[test]
fn existing_send_api_still_follows_redirects() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();

    let server = std::thread::spawn(move || {
        for response in [
            "HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nfinal",
        ] {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request).expect("read");
            stream.write_all(response.as_bytes()).expect("write");
        }
    });

    let start = url(&format!("http://127.0.0.1:{port}/start"));
    let response = send(&FetchRequest::get(start), &HttpConfig::default())
        .expect("redirect-following send succeeds");
    assert_eq!(response.status, 200);
    assert_eq!(response.url.path(), "/final");
    assert_eq!(response.body, b"final");
    assert!(response.redirected);

    server.join().expect("server joins");
}
