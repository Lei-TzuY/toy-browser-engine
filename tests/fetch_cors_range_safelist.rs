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
fn open_ended_range_is_cors_safelisted_and_skips_preflight() {
    let endpoint = "http://api.test/data";
    let (mut browser, transport) = browser_for(
        r#"fetch("http://api.test/data", { headers: { "Range": "bytes=42-" } });"#,
        endpoint,
    );

    assert_eq!(browser.tick().requests_sent, 1);
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method.as_str(), "GET");
    assert_eq!(requests[0].headers.get("range").as_deref(), Some("bytes=42-"));
}

#[test]
fn suffix_range_is_not_cors_safelisted_and_preflights() {
    let endpoint = "http://api.test/data";
    let (mut browser, transport) = browser_for(
        r#"fetch("http://api.test/data", { headers: { "Range": "bytes=-500" } });"#,
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
        Some("range")
    );
}
