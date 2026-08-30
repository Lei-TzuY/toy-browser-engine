use std::rc::Rc;

use browser_engine::browser::Browser;
use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Url};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn cors_response(endpoint: &str, allow_origin: Option<&str>) -> FetchResponse {
    let mut response =
        FetchResponse::synthetic(url(endpoint), 200, Some("text/plain"), b"ok".to_vec());
    if let Some(value) = allow_origin {
        response
            .headers
            .append_raw("access-control-allow-origin", value);
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

#[test]
fn default_cross_origin_fetch_sends_origin_and_accepts_wildcard() {
    let endpoint = "http://api.test/data";
    let (mut browser, transport) = browser_for(
        r#"fetch("http://api.test/data")
               .then(function () { console.log("cors ok"); })
               .catch(function () { console.log("cors failed"); });"#,
        endpoint,
        cors_response(endpoint, Some("*")),
    );

    let first = browser.tick();
    assert_eq!(first.requests_sent, 1);
    let sent = transport.requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].headers.get("origin").as_deref(),
        Some("http://page.test")
    );
    assert!(sent[0].headers.get("cookie").is_none());

    assert_eq!(transport.complete_all(), 1);
    let completion = browser.tick();
    assert_eq!(completion.network_completions, 1);
    assert_eq!(browser.document().runtime.console, vec!["cors ok"]);
}

#[test]
fn cross_origin_fetch_rejects_response_without_acao() {
    let endpoint = "http://api.test/data";
    let (mut browser, transport) = browser_for(
        r#"fetch("http://api.test/data")
               .then(function () { console.log("unexpected"); })
               .catch(function () { console.log("blocked"); });"#,
        endpoint,
        cors_response(endpoint, None),
    );

    browser.tick();
    assert_eq!(
        transport.requests().len(),
        1,
        "CORS is a response gate, not a send gate"
    );
    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(browser.document().runtime.console, vec!["blocked"]);
}

#[test]
fn explicit_same_origin_mode_blocks_before_transport() {
    let endpoint = "http://api.test/data";
    let (browser, transport) = browser_for(
        r#"fetch("http://api.test/data", { mode: "same-origin" })
               .catch(function () { console.log("blocked"); });"#,
        endpoint,
        cors_response(endpoint, Some("*")),
    );

    assert!(transport.requests().is_empty());
    assert_eq!(browser.document().runtime.console, vec!["blocked"]);
}

#[test]
fn request_that_needs_preflight_starts_with_options_not_the_actual_request() {
    let endpoint = "http://api.test/data";
    let (mut browser, transport) = browser_for(
        r#"fetch("http://api.test/data", {
                 method: "PUT",
                 headers: { "X-Token": "secret" },
                 body: "payload"
               }).catch(function () { console.log("blocked"); });"#,
        endpoint,
        cors_response(endpoint, Some("*")),
    );

    assert!(transport.requests().is_empty());
    let turn = browser.tick();
    assert_eq!(turn.requests_sent, 1);
    let sent = transport.requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].method.as_str(), "OPTIONS");
    assert_eq!(
        sent[0]
            .headers
            .get("access-control-request-method")
            .as_deref(),
        Some("PUT")
    );
    assert_eq!(
        sent[0]
            .headers
            .get("access-control-request-headers")
            .as_deref(),
        Some("x-token")
    );
    assert!(browser.document().runtime.console.is_empty());
}

#[test]
fn default_cross_origin_credentials_omit_cookie_send_and_store() {
    let endpoint = "http://api.test/data";
    let mut response = cors_response(endpoint, Some("*"));
    response
        .headers
        .append_raw("set-cookie", "server=new; Path=/");
    let (mut browser, transport) = browser_for(
        r#"fetch("http://api.test/data")
               .then(function () { console.log("done"); });"#,
        endpoint,
        response,
    );

    let jar = browser.cookie_jar();
    assert!(jar
        .borrow_mut()
        .store_set_cookie("session=old; Path=/", &url("http://api.test/"), 0,));

    browser.tick();
    assert!(transport.requests()[0].headers.get("cookie").is_none());
    assert_eq!(transport.complete_all(), 1);
    browser.tick();

    assert_eq!(browser.document().runtime.console, vec!["done"]);
    assert_eq!(
        jar.borrow()
            .get_http_cookie_header(&url(endpoint), 0)
            .as_deref(),
        Some("session=old"),
        "credentials=same-origin must neither send nor absorb cross-origin cookies"
    );
}
