use std::rc::Rc;

use browser_engine::browser::Browser;
use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Url};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn response(
    endpoint: &str,
    allow_origin: &str,
    allow_methods: Option<&str>,
    allow_headers: Option<&str>,
    allow_credentials: bool,
) -> FetchResponse {
    let mut response =
        FetchResponse::synthetic(url(endpoint), 200, Some("text/plain"), b"ok".to_vec());
    response
        .headers
        .append_raw("access-control-allow-origin", allow_origin);
    if let Some(value) = allow_methods {
        response
            .headers
            .append_raw("access-control-allow-methods", value);
    }
    if let Some(value) = allow_headers {
        response
            .headers
            .append_raw("access-control-allow-headers", value);
    }
    if allow_credentials {
        response
            .headers
            .append_raw("access-control-allow-credentials", "true");
    }
    response
}

fn browser_for(
    script: &str,
    endpoint: &str,
    response: FetchResponse,
) -> (Browser, Rc<ManualNetwork>) {
    let page = "http://page.test/index.html";
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

fn complete_preflight_and_send_actual(browser: &mut Browser, transport: &Rc<ManualNetwork>) {
    assert_eq!(transport.complete_all(), 1, "preflight should complete");
    let completion = browser.tick();
    assert_eq!(completion.network_completions, 1);
    assert_eq!(
        transport.requests().len(),
        2,
        "a permitted actual request is dispatched in the preflight completion turn"
    );
}

#[test]
fn put_with_custom_header_preflights_then_sends_actual_request() {
    let endpoint = "http://api.test/data";
    let (mut browser, transport) = browser_for(
        r#"fetch("http://api.test/data", {
     method: "PUT",
     headers: { "X-Token": "secret" },
     body: "payload"
   }).then(function () { console.log("ok"); })
     .catch(function () { console.log("blocked"); });"#,
        endpoint,
        response(endpoint, "*", Some("PUT"), Some("x-token"), false),
    );

    let first = browser.tick();
    assert_eq!(first.requests_sent, 1);
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method.as_str(), "OPTIONS");
    assert_eq!(
        requests[0].headers.get("origin").as_deref(),
        Some("http://page.test")
    );
    assert_eq!(
        requests[0]
            .headers
            .get("access-control-request-method")
            .as_deref(),
        Some("PUT")
    );
    assert_eq!(
        requests[0]
            .headers
            .get("access-control-request-headers")
            .as_deref(),
        Some("x-token")
    );
    assert!(requests[0].headers.get("cookie").is_none());

    complete_preflight_and_send_actual(&mut browser, &transport);
    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].method.as_str(), "PUT");
    assert_eq!(
        requests[1].headers.get("x-token").as_deref(),
        Some("secret")
    );
    assert_eq!(
        requests[1].headers.get("origin").as_deref(),
        Some("http://page.test")
    );
    assert!(requests[1]
        .headers
        .get("access-control-request-method")
        .is_none());
    assert!(requests[1]
        .headers
        .get("access-control-request-headers")
        .is_none());

    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(browser.document().runtime.console, vec!["ok"]);
}

#[test]
fn denied_preflight_never_sends_the_actual_request() {
    let endpoint = "http://api.test/data";
    let (mut browser, transport) = browser_for(
        r#"fetch("http://api.test/data", {
     method: "PUT",
     headers: { "X-Token": "secret" },
     body: "payload"
   }).catch(function () { console.log("blocked"); });"#,
        endpoint,
        response(endpoint, "*", Some("GET"), Some("x-token"), false),
    );

    browser.tick();
    assert_eq!(transport.requests().len(), 1);
    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    browser.tick();

    assert_eq!(transport.requests().len(), 1);
    assert_eq!(browser.document().runtime.console, vec!["blocked"]);
}

#[test]
fn credentialed_preflight_omits_cookie_then_actual_request_includes_it() {
    let endpoint = "http://page.test:8080/data";
    let mut allowed = response(
        endpoint,
        "http://page.test",
        Some("PUT"),
        Some("x-token"),
        true,
    );
    allowed
        .headers
        .append_raw("set-cookie", "server=new; Path=/");
    let (mut browser, transport) = browser_for(
        r#"fetch("http://page.test:8080/data", {
     method: "PUT",
     headers: { "X-Token": "secret" },
     body: "payload",
     credentials: "include"
   }).then(function () { console.log("ok"); });"#,
        endpoint,
        allowed,
    );

    let jar = browser.cookie_jar();
    assert!(jar.borrow_mut().store_set_cookie(
        "sid=abc; Path=/",
        &url("http://page.test:8080/"),
        0,
    ));

    browser.tick();
    let requests = transport.requests();
    assert_eq!(requests[0].method.as_str(), "OPTIONS");
    assert!(requests[0].headers.get("cookie").is_none());

    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(
        jar.borrow()
            .get_http_cookie_header(&url(endpoint), 0)
            .as_deref(),
        Some("sid=abc"),
        "preflight credentials are omitted, including response Set-Cookie"
    );

    browser.tick();
    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].headers.get("cookie").as_deref(),
        Some("sid=abc")
    );

    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    let stored = jar
        .borrow()
        .get_http_cookie_header(&url(endpoint), 0)
        .unwrap_or_default();
    assert!(stored.contains("sid=abc"));
    assert!(stored.contains("server=new"));
    assert_eq!(browser.document().runtime.console, vec!["ok"]);
}

#[test]
fn noncredentialed_preflight_accepts_wildcard_method_and_headers() {
    let endpoint = "http://api.test/data";
    let (mut browser, transport) = browser_for(
        r#"fetch("http://api.test/data", {
     method: "PATCH",
     headers: { "X-Token": "secret" },
     body: "payload"
   }).then(function () { console.log("ok"); });"#,
        endpoint,
        response(endpoint, "*", Some("*"), Some("*"), false),
    );

    browser.tick();
    complete_preflight_and_send_actual(&mut browser, &transport);
    assert_eq!(transport.requests()[1].method.as_str(), "PATCH");
    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(browser.document().runtime.console, vec!["ok"]);
}

#[test]
fn credentialed_preflight_does_not_treat_method_or_header_wildcards_as_permission() {
    let endpoint = "http://page.test:8080/data";
    let (mut browser, transport) = browser_for(
        r#"fetch("http://page.test:8080/data", {
     method: "PUT",
     headers: { "X-Token": "secret" },
     body: "payload",
     credentials: "include"
   }).catch(function () { console.log("blocked"); });"#,
        endpoint,
        response(endpoint, "http://page.test", Some("*"), Some("*"), true),
    );

    browser.tick();
    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    browser.tick();

    assert_eq!(transport.requests().len(), 1);
    assert_eq!(browser.document().runtime.console, vec!["blocked"]);
}
