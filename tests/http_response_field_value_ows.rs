use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

use browser_engine::net::fetch::FetchRequest;
use browser_engine::net::http::{send_once, HttpConfig};
use browser_engine::net::url::Url;

#[test]
fn wire_response_trims_only_http_ows_and_preserves_unicode_whitespace() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
    let address = listener.local_addr().expect("listener address");

    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept request");
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request).expect("read request");

        let response = concat!(
            "HTTP/1.1 200 OK\r\n",
            "X-Ascii-Ows:\t value \t\r\n",
            "X-Unicode-Ows: \u{00a0}value\u{00a0} \r\n",
            "Content-Length: 0\r\n",
            "Connection: close\r\n",
            "\r\n"
        );
        stream
            .write_all(response.as_bytes())
            .expect("write response");
    });

    let url = Url::parse(&format!("http://{address}/headers")).expect("test URL");
    let response = send_once(&FetchRequest::get(url), &HttpConfig::default())
        .expect("HTTP response should parse");

    assert_eq!(
        response.headers.get("x-ascii-ows").as_deref(),
        Some("value")
    );
    assert_eq!(
        response.headers.get("x-unicode-ows").as_deref(),
        Some("\u{00a0}value\u{00a0}")
    );

    server.join().expect("test server thread");
}
