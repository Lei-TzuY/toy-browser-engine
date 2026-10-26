//! A page's `<base href>` changes how its relative references resolve. It must
//! not change *who the page is*: the origin `fetch()` is checked against, and
//! what `location` and `document.cookie` report, belong to the URL the
//! document was actually loaded from.
//!
//! Every file here is synthetic, created in a fresh directory under the system
//! temp dir and removed when the test ends; every server listens on
//! `127.0.0.1` only.

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::rc::Rc;
use std::thread::JoinHandle;

use browser_engine::{
    browser::Browser,
    eventloop::ManualClock,
    net::{DefaultLoader, MemoryLoader, Url},
};

// ── Fixtures ──────────────────────────────────────────────────────────────────

/// A temp directory that removes itself, so a failing test leaves nothing.
///
/// `root/site/` holds the page and `root/site/sub/` a file it may read;
/// `root/outside/` is beyond its reach.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!(
            "browser-engine-document-origin-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("site/sub")).unwrap();
        fs::create_dir_all(root.join("outside")).unwrap();
        fs::write(root.join("site/sub/inner.txt"), "synthetic inner").unwrap();
        fs::write(root.join("outside/beyond.txt"), "synthetic beyond").unwrap();
        Fixture { root }
    }

    fn url(&self, relative: &str) -> Url {
        Url::from_file_path(self.root.join(relative))
    }

    /// Write `site/page.html` and open it from disk.
    fn open(&self, html: &str) -> Browser {
        fs::write(self.root.join("site/page.html"), html).unwrap();
        open(DefaultLoader::new(), &self.url("site/page.html"))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn open(loader: DefaultLoader, url: &Url) -> Browser {
    let mut browser = Browser::open_with_clock(Box::new(loader), url, Rc::new(ManualClock::new()))
        .expect("page loads");
    browser.document_mut().runtime.quiet = true;
    browser
}

/// A page whose head is `head` and whose only script logs what
/// `fetch(reference)` produced.
fn page_fetching(head: &str, reference: &str) -> String {
    page_fetching_with(head, reference, "{}")
}

/// As [`page_fetching`], with `init` as the fetch's second argument.
fn page_fetching_with(head: &str, reference: &str, init: &str) -> String {
    format!(
        r#"<head>{head}</head>
           <script>
             fetch("{reference}", {init})
                 .then(function (r) {{ return r.text().then(function (t) {{
                     console.log(r.status + " " + r.url + " " + t); }}); }},
                       function (e) {{ console.log("rejected: " + e); }});
           </script>"#
    )
}

fn logs_after_network(browser: &mut Browser) -> String {
    browser.settle_network(50);
    browser.document().runtime.console.join("\n")
}

/// A server that answers at most one request with `response`.
///
/// `finish` reports what the page sent, or `None` if it sent nothing: the
/// test connects once itself after the page is done, so a server the page
/// never contacted still wakes up and stops.
struct OneShotServer {
    port: u16,
    worker: JoinHandle<String>,
}

impl OneShotServer {
    fn answering(response: String) -> OneShotServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("local address").port();
        let worker = std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return String::new();
            };
            let head = read_head(&mut stream);
            let _ = stream.write_all(response.as_bytes());
            head
        });
        OneShotServer { port, worker }
    }

    fn url(&self, path: &str) -> Url {
        Url::parse(&format!("http://127.0.0.1:{}{path}", self.port)).expect("valid URL")
    }

    fn finish(self) -> Option<String> {
        // Harmless if the page already used the one answer: the listener is
        // gone and this connection is simply refused.
        if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", self.port)) {
            let _ = stream.write_all(b"GET /test-sentinel HTTP/1.1\r\n\r\n");
        }
        let head = self.worker.join().expect("server thread");
        (!head.starts_with("GET /test-sentinel")).then_some(head)
    }
}

fn read_head(stream: &mut TcpStream) -> String {
    let mut raw = Vec::new();
    let mut byte = [0u8; 1];
    while !raw.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(0) | Err(_) => break,
            Ok(_) => raw.push(byte[0]),
        }
    }
    String::from_utf8_lossy(&raw).trim_end().to_string()
}

fn ok_text(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
}

// ── file: pages ───────────────────────────────────────────────────────────────

#[test]
fn a_base_href_does_not_move_a_local_page_out_of_its_directory() {
    let fixture = Fixture::new("sibling");
    let mut browser = fixture.open(&page_fetching(r#"<base href="../outside/">"#, "beyond.txt"));

    let logs = logs_after_network(&mut browser);
    assert!(logs.starts_with("rejected: TypeError"), "{logs}");
    assert!(!logs.contains("synthetic beyond"), "{logs}");
}

#[test]
fn a_base_href_at_the_filesystem_root_grants_no_wider_scope() {
    let fixture = Fixture::new("root");
    // `<base href="/">` would make the page's "directory" the whole disk. The
    // target is still a synthetic file, named by absolute URL.
    let target = fixture.url("outside/beyond.txt").to_string();
    let mut browser = fixture.open(&page_fetching(r#"<base href="/">"#, &target));

    let logs = logs_after_network(&mut browser);
    assert!(logs.starts_with("rejected: TypeError"), "{logs}");
    assert!(!logs.contains("synthetic beyond"), "{logs}");
}

#[test]
fn a_base_href_inside_the_directory_still_resolves_fetches() {
    let fixture = Fixture::new("inside");
    let mut browser = fixture.open(&page_fetching(r#"<base href="sub/">"#, "inner.txt"));

    let expected = fixture.url("site/sub/inner.txt");
    assert_eq!(
        logs_after_network(&mut browser),
        format!("200 {expected} synthetic inner")
    );
}

// ── http: pages ───────────────────────────────────────────────────────────────

#[test]
fn a_base_href_on_another_port_does_not_make_that_origin_fetchable() {
    // Two loopback listeners on different ports are two origins.
    let page_origin = OneShotServer::answering(ok_text("page origin"));
    let other_origin = OneShotServer::answering(ok_text("synthetic other origin"));
    let page_url = page_origin.url("/index.html");

    // The document comes from memory, so the only network traffic is the
    // page's own fetch.
    // Same-origin mode: the base's origin is not the page's, so nothing is
    // sent there at all. (In the default CORS mode a request may be sent; see
    // the next test for what it must carry.)
    let mut memory = MemoryLoader::new();
    memory.insert(
        page_url.to_string(),
        page_fetching_with(
            &format!(r#"<base href="{}">"#, other_origin.url("/")),
            "secret.txt",
            r#"{ mode: "same-origin" }"#,
        ),
    );
    let mut browser = open(DefaultLoader::new().with_memory(memory), &page_url);

    let logs = logs_after_network(&mut browser);
    assert!(logs.starts_with("rejected: TypeError"), "{logs}");
    assert!(!logs.contains("synthetic other origin"), "{logs}");

    assert_eq!(
        other_origin.finish(),
        None,
        "the other origin must not receive a request"
    );
    assert_eq!(page_origin.finish(), None);
}

/// In the default CORS mode the request to the base's origin is a CORS
/// request from the *page's* origin: the base does not change who is asking,
/// and without the other origin's opt-in the response stays unreadable.
#[test]
fn a_base_href_on_another_port_is_not_the_cors_origin() {
    let page_origin = OneShotServer::answering(ok_text("page origin"));
    let other_origin = OneShotServer::answering(ok_text("synthetic other origin"));
    let page_url = page_origin.url("/index.html");

    let mut memory = MemoryLoader::new();
    memory.insert(
        page_url.to_string(),
        page_fetching(
            &format!(r#"<base href="{}">"#, other_origin.url("/")),
            "secret.txt",
        ),
    );
    let mut browser = open(DefaultLoader::new().with_memory(memory), &page_url);

    let logs = logs_after_network(&mut browser);
    assert!(logs.starts_with("rejected: TypeError"), "{logs}");
    assert!(!logs.contains("synthetic other origin"), "{logs}");

    let page = format!("origin: http://127.0.0.1:{}", page_origin.port);
    if let Some(request) = other_origin.finish() {
        assert!(
            request.lines().any(|line| line.eq_ignore_ascii_case(&page)),
            "the request must name the page's origin, not the base's: {request}"
        );
    }
    assert_eq!(page_origin.finish(), None);
}

#[test]
fn a_same_origin_base_href_still_resolves_fetches() {
    let server = OneShotServer::answering(ok_text("synthetic static"));
    let page_url = server.url("/index.html");

    let mut memory = MemoryLoader::new();
    memory.insert(
        page_url.to_string(),
        page_fetching(r#"<base href="/static/">"#, "data.txt"),
    );
    let mut browser = open(DefaultLoader::new().with_memory(memory), &page_url);

    assert_eq!(
        logs_after_network(&mut browser),
        format!("200 {} synthetic static", server.url("/static/data.txt"))
    );
    let request = server.finish().expect("the page's own origin is asked");
    assert!(
        request.starts_with("GET /static/data.txt HTTP/1.1"),
        "{request}"
    );
}

// ── What the page reports about itself ────────────────────────────────────────

#[test]
fn location_and_cookies_belong_to_the_document_url_not_the_base() {
    let mut memory = MemoryLoader::new();
    memory.insert(
        "http://page.test/dir/index.html",
        r#"<head><base href="http://elsewhere.test/other/"></head>
           <script>
             console.log(location.href);
             console.log(location.host);
             document.cookie = "k=v";
           </script>"#,
    );
    let browser = open(
        DefaultLoader::new().with_memory(memory),
        &Url::parse("http://page.test/dir/index.html").unwrap(),
    );

    let runtime = &browser.document().runtime;
    assert_eq!(
        runtime.console.join("\n"),
        "http://page.test/dir/index.html\npage.test"
    );
    let jar = runtime.cookie_jar.borrow();
    assert_eq!(
        jar.get_document_cookie(&Url::parse("http://page.test/dir/x").unwrap(), 0),
        "k=v",
        "the cookie belongs to the page's own host"
    );
    assert_eq!(
        jar.get_document_cookie(&Url::parse("http://elsewhere.test/other/x").unwrap(), 0),
        "",
        "the base URL's host gets nothing"
    );
}
