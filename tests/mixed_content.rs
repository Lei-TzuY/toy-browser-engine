//! Mixed-content network tests are strictly offline. The URL's scheme is
//! meaningful, but the endpoints use synthetic loader/ManualNetwork instances.
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use browser_engine::eventloop::ManualClock;
use browser_engine::net::{
    scope::{is_mixed_content, Requester}, FetchError, FetchRequest, FetchResponse,
    LoadError, ManualNetwork, MemoryLoader, Resource, ResourceLoader, Url,
};
use browser_engine::Browser;

fn u(value: &str) -> Url { Url::parse(value).unwrap() }

struct ElementLoader {
    html: String,
    requests: Arc<Mutex<Vec<String>>>,
}
impl ElementLoader {
    fn new(html: &str) -> (Self, Arc<Mutex<Vec<String>>>) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        (Self { html: html.into(), requests: requests.clone() }, requests)
    }
}
impl ResourceLoader for ElementLoader {
    fn load(&self, url: &Url) -> Result<Resource, LoadError> {
        Err(LoadError::NotFound(url.to_string()))
    }
    fn load_response_once(
        &self, request: &FetchRequest
    ) -> Result<Option<FetchResponse>, LoadError> {
        if request.url.path() == "/index.html" {
            return Ok(Some(FetchResponse::synthetic(
                request.url.clone(), 200, Some("text/html"), self.html.as_bytes().to_vec()
            )));
        }
        Ok(None)
    }
    fn fetch_once(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        self.requests.lock().unwrap().push(request.url.to_string());
        if request.url.path() == "/redirect.js" {
            let mut response = FetchResponse::synthetic(
                request.url.clone(), 302, Some("text/plain"), vec![]
            );
            response.headers.insert_raw("location", "http://evil.test/redirected.js");
            return Ok(response);
        }
        let (mime, data): (&str, &[u8]) = match request.url.path() {
            "/safe.js" => ("application/javascript", b"console.log('secure script');"),
            "/safe.css" => ("text/css", b"body { color: green }"),
            _ => ("text/plain", b"bad"),
        };
        Ok(FetchResponse::synthetic(request.url.clone(), 200, Some(mime), data.to_vec()))
    }
}

#[test]
fn requests_are_classified_using_document_origin_not_base_href() {
    let requester = Requester::new(&u("https://page.test/index.html"), None);
    assert!(is_mixed_content(
        &u("https://page.test/index.html"), &u("http://cdn.test/x.js")
    ));
    assert!(requester.authorize(&u("http://cdn.test/x.js")).is_err());
    assert!(requester.authorize(&u("https://cdn.test/x.js")).is_ok());
    assert!(Requester::new(&u("http://page.test/index.html"), None)
        .authorize(&u("http://cdn.test/x.js")).is_ok());
}

#[test]
fn browser_blocks_parser_time_script_css_and_image_before_dispatch() {
    let html = r#"<base href="http://evil.test/">
      <link rel="stylesheet" href="evil.css">
      <script src="evil.js"></script>
      <img src="evil.ppm">
      <script>console.log("inline secure");</script>"#;
    let (loader, requests) = ElementLoader::new(html);
    let browser = Browser::open(
        Box::new(loader), &u("https://page.test/index.html")
    ).expect("HTTPS document must remain usable when resources are blocked");
    assert!(requests.lock().unwrap().is_empty(), "no HTTP bytes may be dispatched");
    assert!(browser.document().runtime.console.iter().any(|line| line.contains("inline secure")));
    assert!(browser.document().diagnostics.iter().any(|d| d.message.contains("mixed content")));
}

#[test]
fn secure_script_and_stylesheet_are_still_dispatched() {
    let html = r#"<link rel="stylesheet" href="/safe.css">
        <script src="/safe.js"></script>"#;
    let (loader, requests) = ElementLoader::new(html);
    let browser = Browser::open(
        Box::new(loader), &u("https://page.test/index.html")
    ).expect("valid secure resources");
    assert_eq!(requests.lock().unwrap().len(), 2);
    assert!(browser.document().runtime.console.iter().any(|line| line.contains("secure script")));
}

#[test]
fn https_element_redirect_to_http_is_blocked_before_second_hop() {
    let (loader, requests) = ElementLoader::new(r#"<script src="/redirect.js"></script>"#);
    let browser = Browser::open(
        Box::new(loader), &u("https://page.test/index.html")
    ).expect("page still commits");
    assert_eq!(requests.lock().unwrap().as_slice(), ["https://page.test/redirect.js"]);
    assert!(browser.document().diagnostics.iter().any(|d| d.message.contains("mixed content")));
}

#[test]
fn crossorigin_element_redirect_to_http_never_reaches_plaintext_transport() {
    // The normal Browser::open path must enforce policy in the separate
    // credentials-aware redirect loop, not merely fail a later CORS check.
    for html in [
        r#"<script crossorigin src="/redirect.js"></script>"#,
        r#"<script crossorigin="use-credentials" src="/redirect.js"></script>"#,
        r#"<link rel="stylesheet" crossorigin href="/redirect.js">"#,
        r#"<img crossorigin src="/redirect.js">"#,
    ] {
        let (loader, requests) = ElementLoader::new(html);
        let browser = Browser::open(
            Box::new(loader), &u("https://page.test/index.html")
        ).expect("HTTPS document survives blocked mixed-content redirect");

        let actual = requests.lock().unwrap().clone();
        assert_eq!(
            actual, ["https://page.test/redirect.js"],
            "HTTP redirect must not dispatch for {html}"
        );
        assert!(
            browser.document().diagnostics.iter().any(|d| d.message.contains("mixed content")),
            "diagnostic must explain blocked redirect for {html}: {:?}",
            browser.document().diagnostics
        );
    }
}

#[test]
fn insecure_top_level_navigation_remains_allowed() {
    let (loader, _) = ElementLoader::new("<p>plain user-directed navigation</p>");
    let browser = Browser::open(
        Box::new(loader), &u("http://page.test/index.html")
    ).expect("top-level HTTP is a navigation, not mixed subresource");
    assert_eq!(browser.url().scheme(), "http");
}

#[test]
fn javascript_fetch_from_https_rejects_http_before_dispatch() {
    let mut loader = MemoryLoader::new();
    loader.insert("https://page.test/index.html",
        r#"<script>fetch("http://evil.test/data")
            .then(function () { console.log("BAD"); })
            .catch(function () { console.log("blocked"); });</script>"#);
    let network = Rc::new(ManualNetwork::new());
    let mut browser = Browser::open_with_network(
        Box::new(loader), network.clone(), &u("https://page.test/index.html"),
        Rc::new(ManualClock::new())
    ).expect("HTTPS document");
    browser.tick();
    assert!(network.requests().is_empty(), "no HTTP Fetch may reach the transport");
    assert!(browser.document().runtime.console.iter().any(|x| x == "blocked"),
        "{:?}", browser.document().runtime.console);
}

#[test]
fn javascript_https_fetch_redirect_is_blocked_before_http_hop() {
    let mut loader = MemoryLoader::new();
    loader.insert("https://page.test/index.html",
        r#"<script>fetch("/first")
            .then(function () { console.log("BAD"); })
            .catch(function () { console.log("blocked"); });</script>"#);
    let network = Rc::new(ManualNetwork::new());
    let mut response = FetchResponse::synthetic(
        u("https://page.test/first"), 302, Some("text/plain"), Vec::new()
    );
    response.headers.insert_raw("location", "http://page.test/secret");
    network.respond("https://page.test/first", response);
    let mut browser = Browser::open_with_single_hop_network(
        Box::new(loader), network.clone(), &u("https://page.test/index.html"),
        Rc::new(ManualClock::new())
    ).expect("HTTPS document");
    browser.tick();
    assert_eq!(network.requests().len(), 1);
    assert_eq!(network.complete_all(), 1);
    browser.tick();
    assert_eq!(network.requests().len(), 1, "HTTP redirect must never be dispatched");
    assert!(browser.document().runtime.console.iter().any(|x| x == "blocked"),
        "{:?}", browser.document().runtime.console);
}
