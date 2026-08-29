use std::io::{Read, Write};
use std::net::TcpListener;

use browser_engine::browser::Browser;
use browser_engine::net::{HttpLoader, LoadError, Url};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn read_request(stream: &mut std::net::TcpStream) -> String {
    let mut request = [0u8; 4096];
    let read = stream.read(&mut request).expect("read request");
    String::from_utf8_lossy(&request[..read]).to_string()
}

#[test]
fn browser_open_applies_redirect_set_cookie_before_next_initial_hop() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();

    let server = std::thread::spawn(move || {
        let (mut first, _) = listener.accept().expect("accept first hop");
        let first_wire = read_request(&mut first);
        assert!(first_wire.starts_with("GET /start HTTP/1.1"));
        assert!(
            !first_wire.to_ascii_lowercase().contains("cookie:"),
            "fresh Browser initial request must not invent cookies: {first_wire}"
        );
        first
            .write_all(
                b"HTTP/1.1 302 Found\r\nLocation: /final\r\nSet-Cookie: bootstrap=ready; Path=/; SameSite=Strict\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .expect("write first redirect");

        let (mut second, _) = listener.accept().expect("accept second hop");
        let second_wire = read_request(&mut second);
        assert!(second_wire.starts_with("GET /final HTTP/1.1"));
        assert!(
            second_wire
                .to_ascii_lowercase()
                .contains("cookie: bootstrap=ready"),
            "redirect cookie must be selected after the first response: {second_wire}"
        );
        let body = b"<!doctype html><title>final</title><p>ok</p>";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            String::from_utf8_lossy(body)
        );
        second.write_all(response.as_bytes()).expect("write final");
    });

    let start = url(&format!("http://127.0.0.1:{port}/start"));
    let browser = Browser::open(Box::new(HttpLoader::default()), &start).expect("open browser");

    assert_eq!(browser.url().to_string(), format!("http://127.0.0.1:{port}/final"));
    assert_eq!(browser.history().len(), 1);
    assert_eq!(browser.history()[0], *browser.url());
    server.join().unwrap();
}

#[test]
fn browser_open_preserves_load_error_semantics_after_redirect_chain() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();

    let server = std::thread::spawn(move || {
        let (mut first, _) = listener.accept().expect("accept first hop");
        let _ = read_request(&mut first);
        first
            .write_all(
                b"HTTP/1.1 302 Found\r\nLocation: /missing\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .expect("write redirect");

        let (mut second, _) = listener.accept().expect("accept final hop");
        let second_wire = read_request(&mut second);
        assert!(second_wire.starts_with("GET /missing HTTP/1.1"));
        second
            .write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: 7\r\nConnection: close\r\n\r\nmissing",
            )
            .expect("write 404");
    });

    let start = url(&format!("http://127.0.0.1:{port}/start"));
    let error = match Browser::open(Box::new(HttpLoader::default()), &start) {
        Ok(_) => panic!("redirected 404 must preserve document-load error semantics"),
        Err(error) => error,
    };

    assert_eq!(
        error,
        LoadError::HttpStatus {
            url: format!("http://127.0.0.1:{port}/missing"),
            status: 404,
        }
    );
    server.join().unwrap();
}
