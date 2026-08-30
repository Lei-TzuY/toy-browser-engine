use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use browser_engine::net::{
    FetchError, FetchRequest, FetchResponse, HttpLoader, LoadError, Resource, ResourceLoader, Url,
};
use browser_engine::script::dom_api;
use browser_engine::Browser;

fn url(input: &str) -> Url {
    Url::parse(input).unwrap()
}

fn text(browser: &Browser, id: &str) -> String {
    let path = dom_api::get_element_by_id(&browser.document().dom, id).expect("element exists");
    dom_api::text_content(dom_api::node_at(&browser.document().dom, &path).unwrap())
}

struct LegacyLoader {
    loads: Arc<AtomicUsize>,
    fetches: Arc<AtomicUsize>,
}

impl ResourceLoader for LegacyLoader {
    fn load(&self, url: &Url) -> Result<Resource, LoadError> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        Ok(Resource::new(
            url.clone(),
            Some("text/html".into()),
            b"<title>Legacy</title>".to_vec(),
        ))
    }

    fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        self.fetches.fetch_add(1, Ordering::SeqCst);
        panic!("Browser::open must not replace a legacy loader's load() with fetch(): {request:?}")
    }
}

#[test]
fn default_load_response_preserves_legacy_loader_behavior() {
    let loads = Arc::new(AtomicUsize::new(0));
    let fetches = Arc::new(AtomicUsize::new(0));
    let loader = LegacyLoader {
        loads: loads.clone(),
        fetches: fetches.clone(),
    };

    let browser = Browser::open(Box::new(loader), &url("http://example.test/start")).unwrap();

    assert_eq!(browser.document().title().as_deref(), Some("Legacy"));
    assert_eq!(loads.load(Ordering::SeqCst), 1);
    assert_eq!(fetches.load(Ordering::SeqCst), 0);
}

#[derive(Clone, Default)]
struct MetadataLoader {
    requests: Arc<Mutex<Vec<FetchRequest>>>,
}

impl MetadataLoader {
    fn requests(&self) -> Vec<FetchRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl ResourceLoader for MetadataLoader {
    fn load(&self, url: &Url) -> Result<Resource, LoadError> {
        Err(LoadError::NotFound(format!(
            "metadata loader expects load_response for initial document: {url}"
        )))
    }

    fn load_response(&self, _url: &Url) -> Result<FetchResponse, LoadError> {
        let final_url = url("https://example.test/home");
        let mut response = FetchResponse::synthetic(
            final_url,
            200,
            Some("text/html"),
            br#"
                <p id="seen"></p>
                <script>
                    document.getElementById("seen").textContent = document.cookie;
                </script>
            "#
            .to_vec(),
        );
        response
            .headers
            .append_raw("set-cookie", "boot=one; Path=/; Secure; SameSite=Lax");
        response
            .headers
            .append_raw("strict-transport-security", "max-age=60");
        Ok(response)
    }

    fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        self.requests.lock().unwrap().push(request.clone());
        Ok(FetchResponse::synthetic(
            request.url.clone(),
            200,
            Some("text/html"),
            b"<title>Next</title>".to_vec(),
        ))
    }
}

#[test]
fn initial_response_state_is_installed_before_bootstrap_scripts() {
    let loader = MetadataLoader::default();
    let probe = loader.clone();
    let mut browser = Browser::open(Box::new(loader), &url("http://example.test/start")).unwrap();

    assert_eq!(browser.url().to_string(), "https://example.test/home");
    assert_eq!(
        browser.history()[0].to_string(),
        "https://example.test/home"
    );
    assert_eq!(text(&browser, "seen"), "boot=one");
    assert!(browser
        .local_storage_pool
        .borrow()
        .contains_key("https://example.test"));
    assert!(!browser
        .local_storage_pool
        .borrow()
        .contains_key("http://example.test"));

    browser
        .navigate(&url("http://example.test/next"))
        .expect("learned HSTS applies to the next navigation");

    let requests = probe.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.to_string(), "https://example.test/next");
    assert_eq!(
        requests[0].headers.get("cookie").as_deref(),
        Some("boot=one")
    );
}

#[test]
fn http_loader_load_response_retains_wire_headers() {
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut buffer = [0u8; 1024];
        let _ = stream.read(&mut buffer);
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nSet-Cookie: wire=one; Path=/\r\nStrict-Transport-Security: max-age=60\r\nContent-Length: 13\r\n\r\n<p>served</p>",
            )
            .unwrap();
    });

    let target = url(&format!("http://127.0.0.1:{port}/index.html"));
    let response = HttpLoader::default()
        .load_response(&target)
        .expect("metadata-preserving HTTP load");

    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"<p>served</p>");
    assert_eq!(
        response.headers.get("set-cookie").as_deref(),
        Some("wire=one; Path=/")
    );
    assert_eq!(
        response.headers.get("strict-transport-security").as_deref(),
        Some("max-age=60")
    );
    server.join().unwrap();
}
