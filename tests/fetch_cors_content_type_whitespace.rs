use std::rc::Rc;

use browser_engine::browser::Browser;
use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Url};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn browser_for(script: &str, endpoint: &str) -> (Browser, Rc<ManualNetwork>) {
    let page = "http://page.test/index.html";
    let mut loader = MemoryLoader::new();
    loader.insert(page, format!("<script>{script}</script>"));
    let transport = Rc::new(ManualNetwork::new());
    let mut response =
        FetchResponse::synthetic(url(endpoint), 200, Some("text/plain"), b"ok".to_vec());
    response
        .headers
        .append_raw("access-control-allow-origin", "*");
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
fn ascii_http_whitespace_around_safelisted_content_type_skips_preflight() {
    let endpoint = "http://api.test/data";
    let (mut browser, transport) = browser_for(
        r#"fetch("http://api.test/data", { method: "POST", headers: { "Content-Type": " text/plain " }, body: "ok" });"#,
        endpoint,
    );

    assert_eq!(browser.tick().requests_sent, 1);
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method.as_str(), "POST");
}

#[test]
fn non_http_unicode_whitespace_does_not_make_content_type_safelisted() {
    let endpoint = "http://api.test/data";
    let (mut browser, transport) = browser_for(
        "fetch(\"http://api.test/data\", { method: \"POST\", headers: { \"Content-Type\": \"\\u00a0text/plain\" }, body: \"ok\" });",
        endpoint,
    );

    assert_eq!(browser.tick().requests_sent, 1);
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method.as_str(), "OPTIONS");
    assert_eq!(
        requests[0]
            .headers
            .get("access-control-request-headers")
            .as_deref(),
        Some("content-type")
    );
}
