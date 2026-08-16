//! Fetch against a real HTTP server on the loopback interface.
//!
//! The deterministic tests elsewhere use `ManualNetwork`; these exist to prove
//! the other half — that the bytes really do go out over a socket, that the
//! method, headers and body arrive, and that the answer comes back through the
//! event loop and settles a promise.
//!
//! Nothing here sleeps or guesses. The server binds to port 0 and reports the
//! port it was given, so the client always connects to a listener that is
//! already accepting; the test thread and the server thread meet at
//! `join()`.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread::JoinHandle;

use browser_engine::{
    browser::Browser,
    document::PointerState,
    eventloop::ManualClock,
    net::{
        fetch::{FetchRequest, HeaderMap, Method, NetworkBackend, ThreadedNetwork},
        DefaultLoader, HttpConfig, HttpLoader, ResourceLoader, Url,
    },
    script::dom_api,
};

/// What one request looked like on the wire.
#[derive(Debug, Clone)]
struct Received {
    head: String,
    body: String,
}

impl Received {
    fn request_line(&self) -> &str {
        self.head.lines().next().unwrap_or("")
    }

    fn header(&self, name: &str) -> Option<String> {
        self.head.lines().skip(1).find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim()
                .eq_ignore_ascii_case(name)
                .then(|| value.trim().to_string())
        })
    }
}

/// A one-shot HTTP server: it answers `responses.len()` requests, then stops.
struct TestServer {
    port: u16,
    worker: JoinHandle<Vec<Received>>,
}

impl TestServer {
    /// Serve each canned response in turn.
    fn serving(responses: Vec<String>) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        // The port is known the moment `bind` returns, so the client never has
        // to wait for the server to "be ready".
        let port = listener.local_addr().expect("local address").port();

        let worker = std::thread::spawn(move || {
            let mut received = Vec::new();
            for response in responses {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                received.push(read_request(&mut stream));
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
            received
        });
        TestServer { port, worker }
    }

    fn url(&self, path: &str) -> Url {
        Url::parse(&format!("http://127.0.0.1:{}{path}", self.port)).expect("valid URL")
    }

    /// Wait for the server to finish and report what it saw.
    fn finish(self) -> Vec<Received> {
        self.worker.join().expect("server thread")
    }
}

/// Read one request: the head, then exactly `Content-Length` body bytes.
fn read_request(stream: &mut std::net::TcpStream) -> Received {
    let mut raw: Vec<u8> = Vec::new();
    let mut byte = [0u8; 1];

    // Head, one byte at a time so we stop exactly at the blank line.
    while !raw.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(0) | Err(_) => break,
            Ok(_) => raw.push(byte[0]),
        }
    }
    let head = String::from_utf8_lossy(&raw).trim_end().to_string();

    let length: usize = head
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())
                .flatten()
        })
        .unwrap_or(0);

    let mut body = vec![0u8; length];
    if length > 0 {
        let _ = stream.read_exact(&mut body);
    }
    Received {
        head,
        body: String::from_utf8_lossy(&body).to_string(),
    }
}

fn ok_json(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
}

// ── The client, directly ──────────────────────────────────────────────────────

#[test]
fn a_get_goes_out_over_a_socket_and_the_body_comes_back() {
    let server = TestServer::serving(vec![ok_json(r#"{"message":"from a socket"}"#)]);
    let request = FetchRequest::get(server.url("/api/data.json"));

    let response = HttpLoader::default()
        .fetch(&request)
        .expect("a response, not a network error");

    assert_eq!(response.status, 200);
    assert_eq!(response.status_text, "OK");
    assert_eq!(
        response.headers.get("content-type").as_deref(),
        Some("application/json")
    );
    assert_eq!(
        String::from_utf8_lossy(&response.body),
        r#"{"message":"from a socket"}"#
    );

    let sent = server.finish();
    assert_eq!(sent[0].request_line(), "GET /api/data.json HTTP/1.1");
    assert!(sent[0].header("host").is_some());
}

#[test]
fn a_post_sends_its_headers_and_body() {
    let server = TestServer::serving(vec![ok_json(r#"{"echoed":true}"#)]);
    let mut headers = HeaderMap::new();
    headers.set("content-type", "application/json").unwrap();
    headers.set("x-token", "abc123").unwrap();

    let request = FetchRequest::new(
        server.url("/echo"),
        Method::Post,
        headers,
        Some(br#"{"name":"toy"}"#.to_vec()),
    );
    let response = HttpLoader::default().fetch(&request).expect("a response");
    assert_eq!(response.status, 200);

    let sent = server.finish();
    assert_eq!(sent[0].request_line(), "POST /echo HTTP/1.1");
    assert_eq!(
        sent[0].header("content-type").as_deref(),
        Some("application/json")
    );
    assert_eq!(sent[0].header("x-token").as_deref(), Some("abc123"));
    assert_eq!(sent[0].header("content-length").as_deref(), Some("14"));
    assert_eq!(sent[0].body, r#"{"name":"toy"}"#);
}

#[test]
fn a_head_request_keeps_the_headers_and_drops_the_body() {
    let server = TestServer::serving(vec![
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 5\r\n\r\nhello".to_string(),
    ]);
    let mut request = FetchRequest::get(server.url("/page.html"));
    request.method = Method::Head;

    let response = HttpLoader::default().fetch(&request).expect("a response");
    assert_eq!(response.status, 200);
    assert_eq!(
        response.headers.get("content-type").as_deref(),
        Some("text/html")
    );
    assert!(response.body.is_empty(), "a HEAD has no body");

    assert_eq!(
        server.finish()[0].request_line(),
        "HEAD /page.html HTTP/1.1"
    );
}

#[test]
fn an_error_status_is_a_response_not_a_failure() {
    for (status, phrase) in [(404, "Not Found"), (500, "Internal Server Error")] {
        let body = format!("the {status} body");
        let server = TestServer::serving(vec![format!(
            "HTTP/1.1 {status} {phrase}\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )]);

        let response = HttpLoader::default()
            .fetch(&FetchRequest::get(server.url("/x")))
            .expect("a response, because only the network can fail");
        assert_eq!(response.status, status);
        assert_eq!(response.status_text, phrase);
        assert!(!response.ok());
        assert_eq!(String::from_utf8_lossy(&response.body), body);
        server.finish();
    }
}

#[test]
fn redirects_are_followed_and_reported() {
    let server = TestServer::serving(vec![
        "HTTP/1.1 302 Found\r\nLocation: /second\r\nContent-Length: 0\r\n\r\n".to_string(),
        "HTTP/1.1 302 Found\r\nLocation: /third\r\nContent-Length: 0\r\n\r\n".to_string(),
        ok_json(r#"{"stop":"here"}"#),
    ]);

    let response = HttpLoader::default()
        .fetch(&FetchRequest::get(server.url("/first")))
        .expect("a response");

    assert_eq!(response.status, 200);
    assert_eq!(response.url.path(), "/third", "the final URL is reported");
    assert!(response.redirected);

    let sent = server.finish();
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[2].request_line(), "GET /third HTTP/1.1");
}

#[test]
fn a_post_becomes_a_get_when_it_is_redirected() {
    let server = TestServer::serving(vec![
        "HTTP/1.1 303 See Other\r\nLocation: /result\r\nContent-Length: 0\r\n\r\n".to_string(),
        ok_json("{}"),
    ]);
    let request = FetchRequest::new(
        server.url("/submit"),
        Method::Post,
        HeaderMap::new(),
        Some(b"a=1".to_vec()),
    );

    assert!(HttpLoader::default().fetch(&request).is_ok());
    let sent = server.finish();
    assert_eq!(sent[0].request_line(), "POST /submit HTTP/1.1");
    assert_eq!(
        sent[1].request_line(),
        "GET /result HTTP/1.1",
        "the body does not follow a 303"
    );
    assert!(sent[1].body.is_empty());
}

#[test]
fn a_redirect_loop_stops_at_the_limit() {
    let hop = "HTTP/1.1 302 Found\r\nLocation: /loop\r\nContent-Length: 0\r\n\r\n".to_string();
    let config = HttpConfig {
        max_redirects: 2,
        ..HttpConfig::default()
    };
    // One more response than the client will ask for, so the server never
    // blocks the test by waiting for a connection that does not come.
    let server = TestServer::serving(vec![hop.clone(), hop.clone(), hop]);

    let loader = HttpLoader { config };
    let error = loader
        .fetch(&FetchRequest::get(server.url("/loop")))
        .expect_err("a loop cannot produce a response");
    assert!(error.to_string().contains("too many redirects"), "{error}");
}

#[test]
fn a_refused_connection_is_a_network_error() {
    // Bind and drop, so the port is free and nothing is listening on it.
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let url = Url::parse(&format!("http://127.0.0.1:{port}/gone")).unwrap();

    let error = HttpLoader::default()
        .fetch(&FetchRequest::get(url))
        .expect_err("nothing is listening");
    assert!(error.to_string().starts_with("TypeError"), "{error}");
}

#[test]
fn a_malformed_response_is_a_network_error() {
    let server = TestServer::serving(vec!["this is not HTTP at all".to_string()]);
    let error = HttpLoader::default()
        .fetch(&FetchRequest::get(server.url("/x")))
        .expect_err("garbage is not a response");
    assert!(error.to_string().contains("malformed"), "{error}");
    server.finish();
}

#[test]
fn https_is_refused_with_an_explanation_rather_than_downgraded() {
    let error = HttpLoader::default()
        .fetch(&FetchRequest::get(
            Url::parse("https://example.com/").unwrap(),
        ))
        .expect_err("there is no TLS stack");
    assert!(
        error.to_string().contains("https is not supported"),
        "{error}"
    );
}

// ── The threaded backend ──────────────────────────────────────────────────────

#[test]
fn the_threaded_backend_answers_off_the_browser_thread() {
    let server = TestServer::serving(vec![ok_json(r#"{"threaded":true}"#)]);
    let url = server.url("/api");

    let network = ThreadedNetwork::new(std::sync::Arc::new(DefaultLoader::new()));
    network.start(7, FetchRequest::get(url));

    // `start` returned immediately; the answer arrives on a later poll. Waiting
    // for the server thread is what synchronises us, not a sleep.
    let sent = server.finish();
    assert_eq!(sent[0].request_line(), "GET /api HTTP/1.1");

    let completion = poll_until_delivered(&network);
    assert_eq!(completion.id, 7);
    let response = completion.result.expect("a response");
    assert_eq!(response.status, 200);
    assert_eq!(
        String::from_utf8_lossy(&response.body),
        r#"{"threaded":true}"#
    );
}

#[test]
fn a_cancelled_request_never_reaches_the_page() {
    let server = TestServer::serving(vec![ok_json("{}")]);
    let network = ThreadedNetwork::new(std::sync::Arc::new(DefaultLoader::new()));
    network.start(1, FetchRequest::get(server.url("/api")));
    network.cancel(1);
    server.finish();

    // Poll until the worker has definitely finished, then check nothing came
    // through: `is_busy` goes false only once the thread has exited.
    while network.is_busy() {
        assert!(
            network.poll().is_empty(),
            "a cancelled completion must be dropped"
        );
    }
    assert!(network.poll().is_empty());
}

/// Poll until the worker delivers. `is_busy` tracks the thread itself, so this
/// terminates without a timeout or a sleep.
fn poll_until_delivered(network: &ThreadedNetwork) -> browser_engine::net::FetchCompletion {
    loop {
        if let Some(completion) = network.poll().into_iter().next() {
            return completion;
        }
        assert!(network.is_busy(), "the worker exited without answering");
    }
}

// ── The whole browser, over a socket ──────────────────────────────────────────

#[test]
fn a_page_fetches_json_over_http_and_renders_it() {
    let server = TestServer::serving(vec![ok_json(
        r#"{"message":"served over TCP","items":["alpha","beta"]}"#,
    )]);
    let page_url = server.url("/index.html");

    // The document itself is served from memory; only the fetch goes to the
    // socket, which keeps the test to the one exchange the server offers.
    let mut memory = browser_engine::net::MemoryLoader::new();
    memory.insert(
        page_url.to_string(),
        r#"<p id="status">idle</p><ul id="items"></ul>
           <script>
             fetch("/api/data.json")
                 .then(function (response) {
                     console.log("status " + response.status);
                     return response.json();
                 })
                 .then(function (data) {
                     document.getElementById("status").textContent = data.message;
                     for (const label of data.items) {
                         const row = document.createElement("li");
                         row.textContent = label;
                         document.getElementById("items").appendChild(row);
                     }
                 })
                 .catch(function (e) {
                     document.getElementById("status").textContent = "failed: " + e;
                 });
           </script>"#,
    );

    let mut browser = Browser::open_with_clock(
        Box::new(DefaultLoader::new().with_memory(memory)),
        &page_url,
        Rc::new(ManualClock::new()),
    )
    .expect("page loads");
    browser.document_mut().runtime.quiet = true;

    assert_eq!(text_of(&browser, "#status"), "idle");
    let before = browser
        .render(400, 300, 0.0, &PointerState::default())
        .to_ppm();

    // Turn the loop until the socket answers. `settle_network` stops as soon
    // as nothing is outstanding, so this cannot spin.
    let report = browser.settle_network(200);
    assert_eq!(report.requests_sent, 1);
    assert_eq!(report.network_completions, 1);

    assert_eq!(text_of(&browser, "#status"), "served over TCP");
    assert_eq!(
        dom_api::query_selector_all(&browser.document().dom, &[], "#items li").len(),
        2
    );
    let after = browser
        .render(400, 300, 0.0, &PointerState::default())
        .to_ppm();
    assert_ne!(before, after, "the fetched data must reach the pixels");

    let sent = server.finish();
    assert_eq!(sent[0].request_line(), "GET /api/data.json HTTP/1.1");
}

#[test]
fn a_page_posts_json_over_http_and_reads_the_answer() {
    let server = TestServer::serving(vec![ok_json(r#"{"saved":true,"id":17}"#)]);
    let page_url = server.url("/form.html");

    let mut memory = browser_engine::net::MemoryLoader::new();
    memory.insert(
        page_url.to_string(),
        r#"<p id="result">unsent</p>
           <script>
             fetch("/api/save", {
                 method: "POST",
                 headers: { "Content-Type": "application/json" },
                 body: JSON.stringify({ name: "toy browser" })
             })
                 .then(function (r) { return r.json(); })
                 .then(function (data) {
                     document.getElementById("result").textContent = "saved id " + data.id;
                 })
                 .catch(function (e) {
                     document.getElementById("result").textContent = "failed: " + e;
                 });
           </script>"#,
    );

    let mut browser = Browser::open_with_clock(
        Box::new(DefaultLoader::new().with_memory(memory)),
        &page_url,
        Rc::new(ManualClock::new()),
    )
    .expect("page loads");
    browser.document_mut().runtime.quiet = true;
    browser.settle_network(200);

    assert_eq!(text_of(&browser, "#result"), "saved id 17");

    let sent = server.finish();
    assert_eq!(sent[0].request_line(), "POST /api/save HTTP/1.1");
    assert_eq!(
        sent[0].header("content-type").as_deref(),
        Some("application/json")
    );
    assert_eq!(sent[0].body, r#"{"name":"toy browser"}"#);
}

#[test]
fn a_page_sees_a_404_as_a_response() {
    let server = TestServer::serving(vec![
        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_string()
    ]);
    let page_url = server.url("/index.html");

    let mut memory = browser_engine::net::MemoryLoader::new();
    memory.insert(
        page_url.to_string(),
        r#"<p id="out">?</p>
           <script>
             fetch("/missing")
                 .then(function (r) {
                     document.getElementById("out").textContent =
                         r.status + " " + r.statusText + " ok=" + r.ok;
                 })
                 .catch(function () {
                     document.getElementById("out").textContent = "REJECTED";
                 });
           </script>"#,
    );

    let mut browser = Browser::open_with_clock(
        Box::new(DefaultLoader::new().with_memory(memory)),
        &page_url,
        Rc::new(ManualClock::new()),
    )
    .expect("page loads");
    browser.document_mut().runtime.quiet = true;
    browser.settle_network(200);

    assert_eq!(text_of(&browser, "#out"), "404 Not Found ok=false");
    server.finish();
}

#[test]
fn a_page_catches_a_connection_failure() {
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let page_url = Url::parse(&format!("http://127.0.0.1:{port}/index.html")).unwrap();

    let mut memory = browser_engine::net::MemoryLoader::new();
    memory.insert(
        page_url.to_string(),
        r#"<p id="out">?</p>
           <script>
             fetch("/api")
                 .then(function () { document.getElementById("out").textContent = "RESOLVED"; })
                 .catch(function (e) { document.getElementById("out").textContent = "caught"; });
           </script>"#,
    );

    let mut browser = Browser::open_with_clock(
        Box::new(DefaultLoader::new().with_memory(memory)),
        &page_url,
        Rc::new(ManualClock::new()),
    )
    .expect("page loads");
    browser.document_mut().runtime.quiet = true;
    browser.settle_network(200);

    assert_eq!(text_of(&browser, "#out"), "caught");
}

#[test]
fn a_custom_response_header_reaches_the_page() {
    let body = r#"{"ok":true}"#;
    let server = TestServer::serving(vec![format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nX-Served-By: test-server\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    )]);
    let page_url = server.url("/index.html");

    let mut memory = browser_engine::net::MemoryLoader::new();
    memory.insert(
        page_url.to_string(),
        r#"<p id="out">?</p>
           <script>
             fetch("/api").then(function (r) {
                 document.getElementById("out").textContent =
                     r.headers.get("x-served-by") + " / " + r.headers.get("Content-Type");
             });
           </script>"#,
    );

    let mut browser = Browser::open_with_clock(
        Box::new(DefaultLoader::new().with_memory(memory)),
        &page_url,
        Rc::new(ManualClock::new()),
    )
    .expect("page loads");
    browser.document_mut().runtime.quiet = true;
    browser.settle_network(200);

    assert_eq!(text_of(&browser, "#out"), "test-server / application/json");
    server.finish();
}

#[test]
fn navigating_away_discards_an_answer_still_on_the_wire() {
    // The server holds the connection until the test releases it, so the
    // navigation is guaranteed to happen while the request is outstanding.
    let (release, wait) = mpsc::channel::<()>();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();

    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let _ = read_request(&mut stream);
        // Only answer once the test says the page has gone.
        wait.recv().ok();
        let body = r#"{"late":true}"#;
        let _ = stream.write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        );
    });

    let first = Url::parse(&format!("http://127.0.0.1:{port}/first.html")).unwrap();
    let second = Url::parse(&format!("http://127.0.0.1:{port}/second.html")).unwrap();

    let mut memory = browser_engine::net::MemoryLoader::new();
    memory.insert(
        first.to_string(),
        r#"<p id="out">first page</p>
           <script>
             fetch("/slow").then(function () {
                 document.getElementById("out").textContent = "THE OLD PAGE RAN";
             });
           </script>"#,
    );
    memory.insert(second.to_string(), r#"<p id="out">second page</p>"#);

    let mut browser = Browser::open_with_clock(
        Box::new(DefaultLoader::new().with_memory(memory)),
        &first,
        Rc::new(ManualClock::new()),
    )
    .expect("page loads");
    browser.document_mut().runtime.quiet = true;

    browser.tick(); // the request goes out
    browser.navigate(&second).expect("second page loads");
    release.send(()).ok(); // now let the server answer
    server.join().expect("server thread");

    browser.settle_network(200);
    assert_eq!(
        text_of(&browser, "#out"),
        "second page",
        "the old page's handler must not run against the new document"
    );
}

fn text_of(browser: &Browser, selector: &str) -> String {
    let path = dom_api::query_selector(&browser.document().dom, &[], selector)
        .unwrap_or_else(|| panic!("no element matched {selector}"));
    dom_api::text_content(dom_api::node_at(&browser.document().dom, &path).expect("node"))
}
