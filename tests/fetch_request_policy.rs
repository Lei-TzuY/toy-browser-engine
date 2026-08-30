use std::rc::Rc;

use browser_engine::browser::Browser;
use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Url};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn response(
    endpoint: &str,
    allow_origin: Option<&str>,
    allow_credentials: Option<&str>,
) -> FetchResponse {
    let mut response = FetchResponse::synthetic(
        url(endpoint),
        200,
        Some("text/plain"),
        b"ok".to_vec(),
    );
    if let Some(value) = allow_origin {
        response
            .headers
            .append_raw("access-control-allow-origin", value);
    }
    if let Some(value) = allow_credentials {
        response
            .headers
            .append_raw("access-control-allow-credentials", value);
    }
    response
}

fn browser_for(
    page: &str,
    script: &str,
    endpoint: &str,
    response: FetchResponse,
) -> (Browser, Rc<ManualNetwork>) {
    let mut loader = MemoryLoader::new();
    loader.insert(page, format!("<script>{script}</script>"));

    let transport = Rc::new(ManualNetwork::new());
    transport.respond(endpoint, response);
    let browser = Browser::open_with_network(
        Box::new(loader),
        transport.clone(),
        &url(page),
        Rc::new(ManualClock::new()),
    )
    .expect("page loads");
    (browser, transport)
}

#[test]
fn request_cors_mode_is_visible_cloned_and_used_by_fetch_request() {
    let page = "http://page.test/index.html";
    let endpoint = "http://api.test/data";
    let (mut browser, transport) = browser_for(
        page,
        r#"
            var original = new Request("http://api.test/data", { mode: "cors" });
            var clone = new Request(original);
            console.log(original.mode);
            console.log(clone.mode);
            fetch(clone)
                .then(function () { console.log("ok"); })
                .catch(function () { console.log("blocked"); });
        "#,
        endpoint,
        response(endpoint, Some("*"), None),
    );

    assert_eq!(browser.document().runtime.console, vec!["cors", "cors"]);
    let sent = browser.tick();
    assert_eq!(sent.requests_sent, 1);
    assert_eq!(
        transport.requests()[0].headers.get("origin").as_deref(),
        Some("http://page.test")
    );

    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(browser.document().runtime.console, vec!["cors", "cors", "ok"]);
}

#[test]
fn cloned_same_origin_mode_stays_blocked_before_transport() {
    let page = "http://page.test/index.html";
    let endpoint = "http://api.test/data";
    let (browser, transport) = browser_for(
        page,
        r#"
            var original = new Request("http://api.test/data", { mode: "same-origin" });
            var clone = new Request(original);
            console.log(clone.mode);
            fetch(clone).catch(function () { console.log("blocked"); });
        "#,
        endpoint,
        response(endpoint, Some("*"), None),
    );

    assert!(transport.requests().is_empty());
    assert_eq!(
        browser.document().runtime.console,
        vec!["same-origin", "blocked"]
    );
}

#[test]
fn fetch_init_can_override_mode_without_mutating_the_request() {
    let page = "http://page.test/index.html";
    let endpoint = "http://api.test/data";
    let (mut browser, transport) = browser_for(
        page,
        r#"
            var request = new Request("http://api.test/data", { mode: "same-origin" });
            fetch(request, { mode: "cors" })
                .then(function () { console.log("ok"); });
            console.log(request.mode);
        "#,
        endpoint,
        response(endpoint, Some("*"), None),
    );

    assert_eq!(browser.document().runtime.console, vec!["same-origin"]);
    assert_eq!(browser.tick().requests_sent, 1);
    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(browser.document().runtime.console, vec!["same-origin", "ok"]);
}

#[test]
fn credentialed_cors_request_sends_and_stores_cookies_with_exact_permission() {
    let page = "http://page.test:8000/index.html";
    let endpoint = "http://page.test:9000/data";
    let mut allowed = response(endpoint, Some("http://page.test:8000"), Some("true"));
    allowed
        .headers
        .append_raw("set-cookie", "server=new; Path=/");

    let (mut browser, transport) = browser_for(
        page,
        r#"
            var original = new Request("http://page.test:9000/data", {
                credentials: "include"
            });
            var clone = new Request(original);
            console.log(original.credentials);
            console.log(clone.credentials);
            fetch(clone)
                .then(function () { console.log("ok"); })
                .catch(function () { console.log("blocked"); });
        "#,
        endpoint,
        allowed,
    );

    let jar = browser.cookie_jar();
    assert!(jar.borrow_mut().store_set_cookie(
        "session=old; Path=/",
        &url("http://page.test:9000/"),
        0,
    ));

    assert_eq!(browser.document().runtime.console, vec!["include", "include"]);
    assert_eq!(browser.tick().requests_sent, 1);
    let sent = transport.requests();
    assert_eq!(sent[0].headers.get("origin").as_deref(), Some("http://page.test:8000"));
    assert_eq!(sent[0].headers.get("cookie").as_deref(), Some("session=old"));

    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(browser.document().runtime.console, vec!["include", "include", "ok"]);

    let stored = jar
        .borrow()
        .get_http_cookie_header(&url(endpoint), 0)
        .unwrap_or_default();
    assert!(stored.contains("session=old"), "{stored}");
    assert!(stored.contains("server=new"), "{stored}");
}

#[test]
fn credentialed_cors_rejects_wildcard_allow_origin() {
    let page = "http://page.test:8000/index.html";
    let endpoint = "http://page.test:9000/data";
    let (mut browser, transport) = browser_for(
        page,
        r#"
            fetch("http://page.test:9000/data", { credentials: "include" })
                .then(function () { console.log("unexpected"); })
                .catch(function () { console.log("blocked"); });
        "#,
        endpoint,
        response(endpoint, Some("*"), Some("true")),
    );

    assert_eq!(browser.tick().requests_sent, 1);
    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(browser.document().runtime.console, vec!["blocked"]);
}

#[test]
fn credentialed_cors_requires_lowercase_true_acac() {
    let page = "http://page.test:8000/index.html";
    let endpoint = "http://page.test:9000/data";
    let (mut browser, transport) = browser_for(
        page,
        r#"
            fetch("http://page.test:9000/data", { credentials: "include" })
                .then(function () { console.log("unexpected"); })
                .catch(function () { console.log("blocked"); });
        "#,
        endpoint,
        response(endpoint, Some("http://page.test:8000"), Some("TRUE")),
    );

    assert_eq!(browser.tick().requests_sent, 1);
    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(browser.document().runtime.console, vec!["blocked"]);
}
