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
        TestServer::serving_on(bind_loopback(), responses)
    }

    /// Serve on a listener bound earlier, for servers whose answers must name
    /// each other's ports.
    fn serving_on(listener: TcpListener, responses: Vec<String>) -> TestServer {
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

fn bind_loopback() -> TcpListener {
    TcpListener::bind("127.0.0.1:0").expect("bind loopback")
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
fn https_is_not_downgraded_to_plaintext() {
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let error = HttpLoader::default()
        .fetch(&FetchRequest::get(Url::parse(&format!("https://127.0.0.1:{port}/")).unwrap()))
        .expect_err("no HTTPS server is listening");
    assert!(error.to_string().starts_with("TypeError"), "{error}");
    assert!(!error.to_string().contains("unsupported scheme"), "{error}");
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

// ── Redirects across origins ──────────────────────────────────────────────────
//
// Two loopback listeners on different ports are two origins. A page served
// from one may fetch only from it; a navigation may go anywhere.

fn redirect(status: u16, location: &str) -> String {
    format!("HTTP/1.1 {status} Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n")
}

/// Finish a server the client may never have contacted.
///
/// A server blocks in `accept()` until its quota of requests arrives, so to
/// prove that nothing came, one sentinel connection releases it and is then
/// left out of what is reported. No timeout, so no guessing.
fn finish_possibly_unused(server: TestServer) -> Vec<Received> {
    const SENTINEL: &str = "GET /__test_sentinel__ HTTP/1.1";
    if let Ok(mut stream) = std::net::TcpStream::connect(("127.0.0.1", server.port)) {
        let _ = stream.write_all(format!("{SENTINEL}\r\n\r\n").as_bytes());
    }
    server
        .finish()
        .into_iter()
        .filter(|received| received.request_line() != SENTINEL)
        .collect()
}

/// A page from `origin`'s memory whose script runs at load.
fn open_page_on(origin: &TestServer, script: &str) -> Browser {
    let page_url = origin.url("/index.html");
    let mut memory = browser_engine::net::MemoryLoader::new();
    memory.insert(
        page_url.to_string(),
        format!(r#"<p id="status">idle</p><script>{script}</script>"#),
    );
    let mut browser = Browser::open_with_clock(
        Box::new(DefaultLoader::new().with_memory(memory)),
        &page_url,
        Rc::new(ManualClock::new()),
    )
    .expect("page loads");
    browser.document_mut().runtime.quiet = true;
    browser
}

const REPORT_TEXT: &str = r#"
    .then(function (r) { return r.text().then(function (t) {
        document.getElementById("status").textContent =
            "read " + r.status + " from " + r.url + " redirected=" + r.redirected + ": " + t;
    }); })
    .catch(function (e) { document.getElementById("status").textContent = "rejected: " + e; });"#;

/// A same-origin-mode fetch never reaches another origin: the hop is refused
/// before it is sent. (The default mode is CORS, where the hop is sent without
/// credentials and its response stays unreadable; see the test below.)
#[test]
fn a_page_fetch_does_not_follow_a_redirect_to_another_origin() {
    let other = TestServer::serving(vec![ok_json(r#"{"secret":"other origin"}"#)]);
    let page_origin =
        TestServer::serving(vec![redirect(302, &other.url("/secret.json").to_string())]);

    let mut browser = open_page_on(
        &page_origin,
        &format!(
            r#"fetch("/api/data.json", {{ mode: "same-origin", headers: {{ "Authorization": "Bearer page-token" }} }}){REPORT_TEXT}"#
        ),
    );
    browser.settle_network(200);

    let status = text_of(&browser, "#status");
    assert!(
        status.starts_with("rejected: TypeError") && status.contains("same-origin policy"),
        "a cross-origin redirect must fail like a cross-origin fetch: {status}"
    );
    assert!(
        !status.contains("other origin"),
        "the other origin's body leaked: {status}"
    );

    let asked = page_origin.finish();
    assert_eq!(asked[0].request_line(), "GET /api/data.json HTTP/1.1");
    let reached = finish_possibly_unused(other);
    assert!(
        reached.is_empty(),
        "the redirect must be refused before it is followed, got {reached:?}"
    );
}

/// In the default CORS mode a cross-origin hop is followed, as Fetch does, but
/// it carries the page's own origin and none of its credentials, and the
/// response stays unreadable unless the other origin opts in.
#[test]
fn a_cors_fetch_redirected_to_another_origin_is_unreadable_and_uncredentialed() {
    let other = TestServer::serving(vec![ok_json(r#"{"secret":"other origin"}"#)]);
    let page_origin =
        TestServer::serving(vec![redirect(302, &other.url("/secret.json").to_string())]);
    let page = format!("http://127.0.0.1:{}", page_origin.port);

    let mut browser = open_page_on(
        &page_origin,
        &format!(
            r#"fetch("/api/data.json", {{ headers: {{ "Authorization": "Bearer page-token" }} }}){REPORT_TEXT}"#
        ),
    );
    browser.settle_network(200);

    let status = text_of(&browser, "#status");
    assert!(status.starts_with("rejected: TypeError"), "{status}");
    assert!(
        !status.contains("other origin"),
        "the other origin's body leaked: {status}"
    );

    page_origin.finish();
    for hop in finish_possibly_unused(other) {
        assert_eq!(
            hop.header("authorization"),
            None,
            "credential forwarded: {hop:?}"
        );
        assert_eq!(hop.header("cookie"), None, "cookie forwarded: {hop:?}");
        assert_eq!(
            hop.header("origin").as_deref(),
            Some(page.as_str()),
            "{hop:?}"
        );
    }
}

#[test]
fn a_page_fetch_still_follows_a_same_origin_redirect() {
    let origin = TestServer::serving(vec![
        redirect(302, "/final.json"),
        ok_json(r#"{"same":"origin"}"#),
    ]);
    let final_url = origin.url("/final.json");

    let mut browser = open_page_on(&origin, &format!(r#"fetch("/start.json"){REPORT_TEXT}"#));
    browser.settle_network(200);

    assert_eq!(
        text_of(&browser, "#status"),
        format!(r#"read 200 from {final_url} redirected=true: {{"same":"origin"}}"#)
    );
    let sent = origin.finish();
    assert_eq!(sent[1].request_line(), "GET /final.json HTTP/1.1");
}

#[test]
fn a_page_fetch_redirect_loop_still_stops_at_the_limit() {
    // A page's fetch is redirected by the Browser session, which applies
    // Fetch's limit (`FETCH_MAX_REDIRECTS`, 20) rather than the transport's.
    // That many hops are followed, so one more answer is asked for.
    let limit = browser_engine::redirect_policy::FETCH_MAX_REDIRECTS;
    let hops = (0..=limit).map(|_| redirect(302, "/loop")).collect();
    let origin = TestServer::serving(hops);

    let mut browser = open_page_on(&origin, &format!(r#"fetch("/loop"){REPORT_TEXT}"#));
    browser.settle_network(200);

    let status = text_of(&browser, "#status");
    assert!(status.contains("too many redirects"), "{status}");
    assert_eq!(origin.finish().len(), usize::from(limit) + 1);
}

#[test]
fn a_navigation_still_follows_a_redirect_to_another_origin() {
    let other = TestServer::serving(vec![
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 39\r\n\r\n\
         <title>Landed</title><p>other site</p>"
            .to_string(),
    ]);
    let landing = other.url("/landing.html");
    let first = TestServer::serving(vec![redirect(301, &landing.to_string())]);

    let browser =
        Browser::open(Box::new(DefaultLoader::new()), &first.url("/moved")).expect("navigates");

    assert_eq!(browser.document().title().as_deref(), Some("Landed"));
    assert_eq!(browser.url(), &landing);
    first.finish();
    other.finish();
}

#[test]
fn credentials_do_not_follow_a_redirect_to_another_origin() {
    let other = TestServer::serving(vec![ok_json("{}")]);
    let landing = other.url("/landing");
    let first = TestServer::serving(vec![redirect(307, &landing.to_string())]);

    let mut headers = HeaderMap::new();
    for (name, value) in [
        ("Authorization", "Bearer secret-token"),
        ("Proxy-Authorization", "Basic c2VjcmV0"),
        ("Cookie", "session=secret"),
        ("Content-Type", "application/json"),
        ("X-Trace", "kept"),
    ] {
        headers.set(name, value).unwrap();
    }
    let request = FetchRequest::new(
        first.url("/start"),
        Method::Post,
        headers,
        Some(br#"{"a":1}"#.to_vec()),
    );

    // No page asked for this, so the redirect is followed — without the keys.
    let response = HttpLoader::default().fetch(&request).expect("a response");
    assert_eq!(response.url, landing);
    assert!(response.redirected);

    let asked = first.finish();
    assert_eq!(
        asked[0].header("authorization").as_deref(),
        Some("Bearer secret-token"),
        "the origin the caller chose does get them"
    );
    let landed = &other.finish()[0];
    assert_eq!(
        landed.request_line(),
        "POST /landing HTTP/1.1",
        "307 keeps the method"
    );
    assert_eq!(landed.body, r#"{"a":1}"#, "307 keeps the body");
    for credential in ["authorization", "proxy-authorization", "cookie"] {
        assert_eq!(
            landed.header(credential),
            None,
            "{credential} was forwarded to another origin"
        );
    }
    assert_eq!(landed.header("x-trace").as_deref(), Some("kept"));
    assert_eq!(
        landed.header("content-type").as_deref(),
        Some("application/json")
    );
}

#[test]
fn credentials_dropped_on_leaving_an_origin_are_not_restored_on_returning() {
    // Both listeners exist before either answers, so each can name the other.
    let (a_listener, b_listener) = (bind_loopback(), bind_loopback());
    let a_port = a_listener.local_addr().unwrap().port();
    let b_port = b_listener.local_addr().unwrap().port();
    let a = TestServer::serving_on(
        a_listener,
        vec![
            redirect(302, &format!("http://127.0.0.1:{b_port}/hop")),
            ok_json("{}"),
        ],
    );
    let b = TestServer::serving_on(
        b_listener,
        vec![redirect(302, &format!("http://127.0.0.1:{a_port}/back"))],
    );

    let mut request = FetchRequest::get(a.url("/start"));
    for (name, value) in [
        ("Authorization", "Bearer secret-token"),
        ("Proxy-Authorization", "Basic c2VjcmV0"),
        ("Cookie", "session=secret"),
        ("X-Trace", "kept"),
    ] {
        request.headers.set(name, value).unwrap();
    }

    let response = HttpLoader::default().fetch(&request).expect("a response");
    assert_eq!(response.url, a.url("/back"), "the chain ends back at A");

    let at_a = a.finish();
    let at_b = b.finish();
    assert_eq!(at_a[0].request_line(), "GET /start HTTP/1.1");
    assert_eq!(
        at_a[0].header("authorization").as_deref(),
        Some("Bearer secret-token"),
        "A, the origin the caller chose, gets them first"
    );
    assert_eq!(at_b[0].request_line(), "GET /hop HTTP/1.1");
    assert_eq!(at_a[1].request_line(), "GET /back HTTP/1.1");
    for credential in ["authorization", "proxy-authorization", "cookie"] {
        assert_eq!(at_b[0].header(credential), None, "{credential} reached B");
        assert_eq!(
            at_a[1].header(credential),
            None,
            "{credential} came back after a detour through B"
        );
    }
    assert_eq!(at_b[0].header("x-trace").as_deref(), Some("kept"));
    assert_eq!(at_a[1].header("x-trace").as_deref(), Some("kept"));
}

#[test]
fn credentials_still_follow_a_same_origin_redirect() {
    let origin = TestServer::serving(vec![redirect(302, "/next"), ok_json("{}")]);
    let mut request = FetchRequest::get(origin.url("/start"));
    request
        .headers
        .set("Authorization", "Bearer secret-token")
        .unwrap();

    HttpLoader::default().fetch(&request).expect("a response");
    let sent = origin.finish();
    assert_eq!(sent[1].request_line(), "GET /next HTTP/1.1");
    assert_eq!(
        sent[1].header("authorization").as_deref(),
        Some("Bearer secret-token")
    );
}

#[test]
fn redirect_status_codes_keep_or_drop_the_method_and_body() {
    use Method::{Delete, Get, Head, Options, Patch, Post, Put};

    // Explicit expected wire methods, in the input order below. In particular,
    // GET/HEAD under 303 retain their existing representation headers even
    // though neither request has a body.
    // https://fetch.spec.whatwg.org/#http-redirect-fetch
    for (status, methods_after) in [
        (
            301,
            ["GET", "HEAD", "GET", "PUT", "DELETE", "PATCH", "OPTIONS"],
        ),
        (
            302,
            ["GET", "HEAD", "GET", "PUT", "DELETE", "PATCH", "OPTIONS"],
        ),
        (303, ["GET", "HEAD", "GET", "GET", "GET", "GET", "GET"]),
        (
            307,
            ["GET", "HEAD", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"],
        ),
        (
            308,
            ["GET", "HEAD", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"],
        ),
    ] {
        for (method, method_after) in [Get, Head, Post, Put, Delete, Patch, Options]
            .into_iter()
            .zip(methods_after)
        {
            let origin = TestServer::serving(vec![redirect(status, "/next"), ok_json("{}")]);
            let body = method
                .allows_body()
                .then(|| "payload=字".as_bytes().to_vec());
            let request = FetchRequest::new(
                origin.url("/form"),
                method,
                redirect_test_headers(),
                body.clone(),
            );

            let response = HttpLoader::default().fetch(&request).expect("a response");
            assert_eq!(response.status, 200);
            assert!(response.redirected);
            assert_eq!(response.url, origin.url("/next"));
            let sent = origin.finish();
            assert_eq!(sent.len(), 2);
            assert_eq!(sent[0].request_line(), format!("{method} /form HTTP/1.1"));
            let original_body = std::str::from_utf8(body.as_deref().unwrap_or_default()).unwrap();
            assert_redirect_payload(&sent[0], original_body, true);
            assert_eq!(
                sent[1].request_line(),
                format!("{method_after} /next HTTP/1.1"),
                "{status} from {method}"
            );
            let retained = method.as_str() == method_after;
            assert_redirect_payload(
                &sent[1],
                if retained { original_body } else { "" },
                retained,
            );
        }
    }
}

const REDIRECT_BODY_HEADERS: &[(&str, &str)] = &[
    ("Content-Type", "text/plain;charset=utf-8"),
    ("Content-Encoding", "identity"),
    ("Content-Language", "en"),
    ("Content-Location", "/representation"),
];

fn redirect_test_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in REDIRECT_BODY_HEADERS {
        headers.set(name, value).unwrap();
    }
    headers.set("X-Trace", "kept").unwrap();
    headers
}

fn assert_redirect_payload(sent: &Received, body: &str, retained: bool) {
    assert_eq!(sent.body, body, "request body: {sent:?}");
    assert_eq!(
        sent.header("content-length"),
        (!body.is_empty()).then(|| body.len().to_string()),
        "body framing: {sent:?}"
    );
    for (name, value) in REDIRECT_BODY_HEADERS {
        assert_eq!(
            sent.header(name).as_deref(),
            retained.then_some(*value),
            "{name}: {sent:?}"
        );
    }
    assert_eq!(sent.header("x-trace").as_deref(), Some("kept"));
}

#[test]
fn redirect_chain_preserves_put_until_303_then_never_restores_the_body() {
    let mut responses: Vec<String> = [301, 302, 307, 308, 303, 307]
        .into_iter()
        .enumerate()
        .map(|(index, status)| redirect(status, &format!("/hop{}", index + 1)))
        .collect();
    responses.push(ok_json("{}"));
    let origin = TestServer::serving(responses);
    let request = FetchRequest::new(
        origin.url("/start"),
        Method::Put,
        redirect_test_headers(),
        Some(b"payload".to_vec()),
    );
    let loader = HttpLoader {
        config: HttpConfig {
            max_redirects: 6,
            ..HttpConfig::default()
        },
    };
    let response = loader.fetch(&request).expect("six redirects are allowed");
    assert_eq!(response.url, origin.url("/hop6"));
    let sent = origin.finish();
    assert_eq!(sent.len(), 7);
    for (index, received) in sent.iter().enumerate() {
        let path = if index == 0 {
            "/start".to_string()
        } else {
            format!("/hop{index}")
        };
        let (method, body, retained) = if index < 5 {
            ("PUT", "payload", true)
        } else {
            ("GET", "", false)
        };
        assert_eq!(received.request_line(), format!("{method} {path} HTTP/1.1"));
        assert_redirect_payload(received, body, retained);
    }
}

#[test]
fn redirect_limit_does_not_rewrite_a_preserved_patch_request() {
    let origin = TestServer::serving(vec![redirect(301, "/loop"); 3]);
    let request = FetchRequest::new(
        origin.url("/loop"),
        Method::Patch,
        redirect_test_headers(),
        Some(b"payload".to_vec()),
    );
    let loader = HttpLoader {
        config: HttpConfig {
            max_redirects: 2,
            ..HttpConfig::default()
        },
    };
    let error = loader
        .fetch(&request)
        .expect_err("redirect budget must be enforced");
    assert!(error.to_string().contains("too many redirects"), "{error}");
    let sent = origin.finish();
    assert_eq!(sent.len(), 3);
    for received in sent {
        assert_eq!(received.request_line(), "PATCH /loop HTTP/1.1");
        assert_redirect_payload(&received, "payload", true);
    }
}

#[test]
fn redirect_method_preservation_does_not_forward_cross_origin_credentials() {
    for (status, method, method_after) in [
        (301, Method::Put, "PUT"),
        (302, Method::Delete, "DELETE"),
        (302, Method::Patch, "PATCH"),
        (303, Method::Put, "GET"),
        (307, Method::Post, "POST"),
        (308, Method::Put, "PUT"),
    ] {
        let target = TestServer::serving(vec![ok_json("{}")]);
        let origin = TestServer::serving(vec![redirect(status, &target.url("/next").to_string())]);
        let mut headers = redirect_test_headers();
        for name in ["Authorization", "Proxy-Authorization", "Cookie"] {
            headers.set(name, "secret").unwrap();
        }
        let request = FetchRequest::new(
            origin.url("/start"),
            method,
            headers,
            Some(b"payload".to_vec()),
        );
        HttpLoader::default()
            .fetch(&request)
            .expect("redirect succeeds");
        let first = origin.finish();
        let second = target.finish();
        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 1);
        assert_eq!(
            second[0].request_line(),
            format!("{method_after} /next HTTP/1.1")
        );
        let retained = method.as_str() == method_after;
        assert_redirect_payload(&second[0], if retained { "payload" } else { "" }, retained);
        for name in ["Authorization", "Proxy-Authorization", "Cookie"] {
            assert_eq!(first[0].header(name).as_deref(), Some("secret"));
            assert_eq!(second[0].header(name), None, "{status} {method}: {name}");
        }
    }
}

// Fetch's request guard belongs at the script boundary. These socket tests
// distinguish it from the transport's smaller set of self-generated headers.
// https://fetch.spec.whatwg.org/#forbidden-request-header
const SCRIPT_FORBIDDEN_NAMES: &[&str] = &[
    "Accept-Charset",
    "Accept-Encoding",
    "Access-Control-Request-Headers",
    "Access-Control-Request-Method",
    "Connection",
    "Content-Length",
    "CoOkIe",
    "cOoKiE2",
    "Date",
    "DNT",
    "Expect",
    "Host",
    "Keep-Alive",
    "oRiGiN",
    "rEfErEr",
    "Set-Cookie",
    "TE",
    "Trailer",
    "Transfer-Encoding",
    "Upgrade",
    "Via",
    "PrOxY-Attacker",
    "sEc-Attacker",
    "Proxy-",
    "Sec-",
];

fn forbidden_header_record() -> String {
    let mut fields: Vec<String> = SCRIPT_FORBIDDEN_NAMES
        .iter()
        .map(|name| format!("'{name}': 'forged'"))
        .collect();
    for name in [
        "X-HTTP-Method",
        "X-HTTP-Method-Override",
        "X-Method-Override",
    ] {
        fields.push(format!("'{name}': 'GET, TrAcE'"));
    }
    fields.push("'X-Allowed': 'kept'".into());
    format!("{{ {} }}", fields.join(", "))
}

fn assert_script_headers_not_forged(sent: &Received) {
    for name in SCRIPT_FORBIDDEN_NAMES {
        assert_ne!(
            sent.header(name).as_deref(),
            Some("forged"),
            "script-controlled {name} reached the socket: {sent:?}"
        );
        // Only these fields can legitimately be regenerated for this GET.
        if !matches!(
            name.to_ascii_lowercase().as_str(),
            "host" | "connection" | "accept-encoding" | "origin" | "referer"
        ) {
            assert_eq!(sent.header(name), None, "{name}: {sent:?}");
        }
    }
    for name in [
        "x-http-method",
        "x-http-method-override",
        "x-method-override",
    ] {
        assert_eq!(sent.header(name), None, "{name}: {sent:?}");
    }
    assert_eq!(sent.header("x-allowed").as_deref(), Some("kept"));
    assert_eq!(sent.header("accept-encoding").as_deref(), Some("identity"));
    assert_eq!(sent.header("connection").as_deref(), Some("close"));
}

#[test]
fn forbidden_script_headers_never_reach_same_origin_sockets() {
    // Exercise the issue's direct fetch(init) path as well as Headers input and
    // Request cloning/mutation. Both CORS and explicit same-origin mode apply
    // the guard before any browser-owned metadata is generated.
    for mode in ["cors", "same-origin"] {
        for input in ["record", "headers", "request"] {
            let origin = TestServer::serving(vec![ok_json("{}")]);
            let record = forbidden_header_record();
            let call = match input {
                "record" => format!("fetch('/api', {{ mode: '{mode}', headers: {record} }})"),
                "headers" => {
                    format!("fetch('/api', {{ mode: '{mode}', headers: new Headers({record}) }})")
                }
                _ => format!(
                    "var r = new Request('/api', {{ mode: '{mode}', headers: {record} }});\
                     r.headers.set('Cookie', 'forged');\
                     r.headers.append('Cookie2', 'forged');\
                     r.headers.set('X-Method-Override', 'CONNECT');\
                     fetch(r.clone())"
                ),
            };
            let mut browser = open_page_on(&origin, &format!("{call}{REPORT_TEXT}"));
            browser.settle_network(200);
            let status = text_of(&browser, "#status");
            assert!(status.starts_with("read 200"), "{mode}/{input}: {status}");
            let received = origin.finish();
            assert_eq!(received.len(), 1);
            assert_eq!(received[0].request_line(), "GET /api HTTP/1.1");
            assert_script_headers_not_forged(&received[0]);
            assert_eq!(
                received[0].header("referer"),
                Some(browser.url().to_string()),
                "browser-generated referrer survives the script guard"
            );
        }
    }
}

#[test]
fn forbidden_script_headers_cannot_forge_cors_preflight_or_metadata() {
    let origin = TestServer::serving(Vec::new());
    let page_origin = format!("http://127.0.0.1:{}", origin.port);
    let target = TestServer::serving(vec![
        format!(
            "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: {page_origin}\r\n\
             Access-Control-Allow-Methods: GET\r\nAccess-Control-Allow-Headers: x-allowed\r\n\r\n"
        ),
        format!(
            "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: {page_origin}\r\n\
             Content-Length: 2\r\n\r\nok"
        ),
    ]);
    let mut browser = open_page_on(
        &origin,
        &format!(
            "fetch('{}', {{ headers: {} }}){REPORT_TEXT}",
            target.url("/api"),
            forbidden_header_record()
        ),
    );
    browser.settle_network(200);
    assert!(text_of(&browser, "#status").starts_with("read 200"));
    origin.finish();
    let received = target.finish();
    assert_eq!(received.len(), 2);
    assert_eq!(received[0].request_line(), "OPTIONS /api HTTP/1.1");
    assert_eq!(
        received[0].header("origin").as_deref(),
        Some(page_origin.as_str())
    );
    assert_eq!(
        received[0]
            .header("access-control-request-method")
            .as_deref(),
        Some("GET")
    );
    assert_eq!(
        received[0]
            .header("access-control-request-headers")
            .as_deref(),
        Some("x-allowed")
    );
    assert_eq!(received[1].request_line(), "GET /api HTTP/1.1");
    assert_script_headers_not_forged(&received[1]);
    assert_eq!(
        received[1].header("origin").as_deref(),
        Some(page_origin.as_str())
    );
    assert_eq!(
        received[1].header("referer"),
        Some(format!("{page_origin}/"))
    );
}

#[test]
fn forbidden_script_origin_cannot_make_a_cross_origin_response_readable() {
    let origin = TestServer::serving(Vec::new());
    let target = TestServer::serving(vec![
        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: http://forged.test\r\n\
         Content-Length: 6\r\n\r\nsecret"
            .to_string(),
    ]);
    let mut browser = open_page_on(
        &origin,
        &format!(
            "fetch('{}', {{ headers: {{ Origin: 'http://forged.test', Cookie: 'forged' }} }}){REPORT_TEXT}",
            target.url("/api")
        ),
    );
    browser.settle_network(200);
    assert!(text_of(&browser, "#status").starts_with("rejected: TypeError"));
    let received = target.finish();
    assert_eq!(
        received.len(),
        1,
        "forbidden fields must not trigger preflight"
    );
    assert_eq!(received[0].header("cookie"), None);
    assert_eq!(
        received[0].header("origin"),
        Some(format!("http://127.0.0.1:{}", origin.port))
    );
    origin.finish();
}

#[test]
fn forbidden_script_header_rules_do_not_strip_network_owned_headers() {
    let origin = TestServer::serving(vec![ok_json("{}")]);
    let mut request = FetchRequest::get(origin.url("/api"));
    for (name, value) in [
        ("Cookie", "session=browser-selected"),
        ("Origin", "http://browser.test"),
        ("Referer", "http://browser.test/page"),
        ("Sec-Fetch-Site", "same-origin"),
        ("Sec-Fetch-Mode", "cors"),
    ] {
        request.headers.insert_raw(name, value);
    }
    HttpLoader::default()
        .fetch(&request)
        .expect("browser-owned request succeeds");
    let received = origin.finish();
    for (name, value) in request.headers.iter() {
        assert_eq!(received[0].header(name).as_deref(), Some(value), "{name}");
    }
}

#[test]
fn forbidden_method_overrides_are_value_sensitive_on_the_wire() {
    for name in [
        "X-HTTP-Method",
        "x-HtTp-MeThOd-OvErRiDe",
        "X-Method-Override",
    ] {
        for (value, allowed) in [
            ("PATCH", true),
            ("GETTRACE", true),
            ("GET,track", false),
            ("cOnNeCt", false),
            ("GET,\tTrAcE\t", false),
            ("\"GET, TRACE, POST\"", true),
            ("GET, \"TRACE\"", true),
        ] {
            let origin = TestServer::serving(vec![ok_json("{}")]);
            let mut browser = open_page_on(
                &origin,
                &format!("fetch('/api', {{ headers: {{ '{name}': '{value}' }} }}){REPORT_TEXT}"),
            );
            browser.settle_network(200);
            assert!(text_of(&browser, "#status").starts_with("read 200"));
            let received = origin.finish();
            assert_eq!(
                received[0].header(name),
                allowed.then(|| value.trim_matches([' ', '\t']).to_string()),
                "{name}: {value:?}"
            );
        }
    }
}
