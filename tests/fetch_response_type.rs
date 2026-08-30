use std::rc::Rc;

use browser_engine::browser::Browser;
use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Url};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn browser_for(script: &str) -> (Browser, Rc<ManualNetwork>) {
    let page = "http://page.test/index.html";
    let mut loader = MemoryLoader::new();
    loader.insert(page, format!("<script>{script}</script>"));
    let transport = Rc::new(ManualNetwork::new());
    let browser = Browser::open_with_network(
        Box::new(loader),
        transport.clone(),
        &url(page),
        Rc::new(ManualClock::new()),
    )
    .expect("page loads");
    (browser, transport)
}

fn response(endpoint: &str) -> FetchResponse {
    FetchResponse::synthetic(
        url(endpoint),
        200,
        Some("text/plain"),
        b"ok".to_vec(),
    )
}

#[test]
fn constructed_response_type_is_basic() {
    let (browser, _) = browser_for(r#"console.log(new Response("ok").type);"#);
    assert_eq!(browser.document().runtime.console, vec!["basic"]);
}

#[test]
fn same_origin_fetch_response_type_is_basic() {
    let endpoint = "http://page.test/data";
    let (mut browser, transport) = browser_for(
        r#"fetch("/data").then(function (response) { console.log(response.type); });"#,
    );
    transport.respond(endpoint, response(endpoint));

    assert_eq!(browser.tick().requests_sent, 1);
    assert_eq!(transport.complete_all(), 1);
    browser.tick();

    assert_eq!(browser.document().runtime.console, vec!["basic"]);
}

#[test]
fn cross_origin_cors_fetch_response_type_is_cors() {
    let endpoint = "http://api.test/data";
    let (mut browser, transport) = browser_for(
        r#"fetch("http://api.test/data").then(function (response) { console.log(response.type); });"#,
    );
    let mut allowed = response(endpoint);
    allowed
        .headers
        .append_raw("access-control-allow-origin", "*");
    transport.respond(endpoint, allowed);

    assert_eq!(browser.tick().requests_sent, 1);
    assert_eq!(transport.complete_all(), 1);
    browser.tick();

    assert_eq!(browser.document().runtime.console, vec!["cors"]);
}

#[test]
fn preflighted_cross_origin_fetch_response_type_is_cors() {
    let endpoint = "http://api.test/data";
    let (mut browser, transport) = browser_for(
        r#"fetch("http://api.test/data", {
               method: "PUT",
               headers: { "X-Token": "secret" },
               body: "payload"
             }).then(function (response) { console.log(response.type); });"#,
    );
    let mut allowed = response(endpoint);
    allowed
        .headers
        .append_raw("access-control-allow-origin", "*");
    allowed
        .headers
        .append_raw("access-control-allow-methods", "PUT");
    allowed
        .headers
        .append_raw("access-control-allow-headers", "x-token");
    transport.respond(endpoint, allowed);

    assert_eq!(browser.tick().requests_sent, 1, "OPTIONS starts first");
    assert_eq!(transport.complete_all(), 1);
    let after_preflight = browser.tick();
    assert_eq!(after_preflight.network_completions, 1);
    assert_eq!(transport.requests().len(), 2, "actual PUT is queued after preflight");

    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(browser.document().runtime.console, vec!["cors"]);
}
