use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::rc::Rc;
use std::time::Duration;

use browser_engine::eventloop::ManualClock;
use browser_engine::net::{
    FetchError, FetchRequest, FetchResponse, HeaderMap, HttpLoader, Method, NetworkBackend,
    ResourceLoader, Url,
};
use browser_engine::SessionNetwork;

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn read_request(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut data = Vec::new();
    loop {
        let mut buffer = [0u8; 2048];
        let count = stream.read(&mut buffer).expect("read request");
        if count == 0 {
            break;
        }
        data.extend_from_slice(&buffer[..count]);

        let Some(split) = data.windows(4).position(|w| w == b"\r\n\r\n") else {
            continue;
        };
        let head = String::from_utf8_lossy(&data[..split]);
        let content_length = head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        if data.len() >= split + 4 + content_length {
            break;
        }
    }
    String::from_utf8_lossy(&data).into_owned()
}

fn write_response(stream: &mut TcpStream, response: &str) {
    stream.write_all(response.as_bytes()).expect("write response");
}

#[test]
fn post_302_becomes_get_and_drops_cookie_and_body_headers() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();

    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();

        let (mut first, _) = listener.accept().expect("first accept");
        requests.push(read_request(&mut first));
        write_response(
            &mut first,
            "HTTP/1.1 302 Found\r\nLocation: /next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        drop(first);

        let (mut second, _) = listener.accept().expect("second accept");
        requests.push(read_request(&mut second));
        write_response(
            &mut second,
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
        );
        requests
    });

    let mut headers = HeaderMap::new();
    headers.insert_raw("cookie", "private=one");
    headers.insert_raw("content-type", "application/x-www-form-urlencoded");
    headers.insert_raw("content-language", "en");
    let request = FetchRequest::new(
        url(&format!("http://127.0.0.1:{port}/start")),
        Method::Post,
        headers,
        Some(b"secret=body".to_vec()),
    );

    let response = HttpLoader::default().fetch(&request).expect("redirect succeeds");
    assert!(response.redirected);
    assert_eq!(response.url.path(), "/next");

    let requests = server.join().expect("server joins");
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("POST /start HTTP/1.1\r\n"));
    assert!(requests[0].contains("cookie: private=one\r\n"));
    assert!(requests[0].contains("secret=body"));

    let second = &requests[1];
    assert!(second.starts_with("GET /next HTTP/1.1\r\n"), "{second}");
    assert!(!second.contains("cookie:"), "{second}");
    assert!(!second.contains("content-type:"), "{second}");
    assert!(!second.contains("content-language:"), "{second}");
    assert!(!second.contains("secret=body"), "{second}");
}

#[test]
fn cross_origin_307_preserves_method_and_body_but_strips_credentials() {
    let target = TcpListener::bind("127.0.0.1:0").expect("bind target");
    let target_port = target.local_addr().unwrap().port();
    let source = TcpListener::bind("127.0.0.1:0").expect("bind source");
    let source_port = source.local_addr().unwrap().port();

    let target_server = std::thread::spawn(move || {
        let (mut stream, _) = target.accept().expect("target accept");
        let request = read_request(&mut stream);
        write_response(
            &mut stream,
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
        );
        request
    });

    let source_server = std::thread::spawn(move || {
        let (mut stream, _) = source.accept().expect("source accept");
        let request = read_request(&mut stream);
        write_response(
            &mut stream,
            &format!(
                "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://127.0.0.1:{target_port}/next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            ),
        );
        request
    });

    let mut headers = HeaderMap::new();
    headers.insert_raw("cookie", "session=secret");
    headers.insert_raw("authorization", "Bearer top-secret");
    headers.insert_raw("proxy-authorization", "Basic proxy-secret");
    headers.insert_raw("content-type", "text/plain");
    headers.insert_raw("x-trace", "keep-me");
    let request = FetchRequest::new(
        url(&format!("http://127.0.0.1:{source_port}/start")),
        Method::Post,
        headers,
        Some(b"payload".to_vec()),
    );

    let response = HttpLoader::default().fetch(&request).expect("redirect succeeds");
    assert!(response.redirected);

    let source_request = source_server.join().expect("source joins");
    assert!(source_request.contains("cookie: session=secret\r\n"));
    assert!(source_request.contains("authorization: Bearer top-secret\r\n"));

    let target_request = target_server.join().expect("target joins");
    assert!(
        target_request.starts_with("POST /next HTTP/1.1\r\n"),
        "{target_request}"
    );
    assert!(!target_request.contains("cookie:"), "{target_request}");
    assert!(!target_request.contains("authorization:"), "{target_request}");
    assert!(
        !target_request.contains("proxy-authorization:"),
        "{target_request}"
    );
    assert!(target_request.contains("content-type: text/plain\r\n"));
    assert!(target_request.contains("x-trace: keep-me\r\n"));
    assert!(target_request.contains("\r\n\r\npayload"));
}

#[test]
fn session_network_blocks_cross_origin_redirect_before_cookie_and_hsts_absorption() {
    let transport = Rc::new(browser_engine::net::ManualNetwork::new());
    transport.set_auto_complete(true);

    let mut redirected = FetchResponse::synthetic(
        url("https://other.test/final"),
        200,
        Some("text/plain"),
        b"secret cross-origin body".to_vec(),
    );
    redirected.redirected = true;
    redirected
        .headers
        .append_raw("set-cookie", "cross=bad; Path=/; Secure; SameSite=None");
    redirected
        .headers
        .append_raw("strict-transport-security", "max-age=3600");
    transport.respond("http://example.test/start", redirected);

    let clock = Rc::new(ManualClock::new());
    let session = SessionNetwork::with_new_state(transport, clock);
    session.start(1, FetchRequest::get(url("http://example.test/start")));

    let completions = session.poll();
    assert_eq!(completions.len(), 1);
    assert!(matches!(
        completions[0].result,
        Err(FetchError::Blocked(_))
    ));
    assert_eq!(session.cookie_jar().borrow().len(), 0);
    assert!(!session.hsts_cache().borrow().is_known_host("other.test", 0));
}

#[test]
fn session_network_allows_same_origin_redirect_and_keeps_response_policy() {
    let transport = Rc::new(browser_engine::net::ManualNetwork::new());
    transport.set_auto_complete(true);

    let mut redirected = FetchResponse::synthetic(
        url("http://example.test/final"),
        200,
        Some("text/plain"),
        b"ok".to_vec(),
    );
    redirected.redirected = true;
    redirected
        .headers
        .append_raw("set-cookie", "same=good; Path=/; SameSite=Lax");
    transport.respond("http://example.test/start", redirected);

    let clock = Rc::new(ManualClock::new());
    let session = SessionNetwork::with_new_state(transport, clock);
    session.start(1, FetchRequest::get(url("http://example.test/start")));

    let completions = session.poll();
    assert_eq!(completions.len(), 1);
    assert!(completions[0].result.is_ok());
    assert_eq!(session.cookie_jar().borrow().len(), 1);
}
