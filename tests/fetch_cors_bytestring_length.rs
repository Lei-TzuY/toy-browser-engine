use std::rc::Rc;

use browser_engine::browser::Browser;
use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Url};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn browser_for(script: &str, endpoint: &str) -> (Browser, Rc<ManualNetwork>) {
    let page = "http://page.test/index.html";
    let script_url = "http://page.test/app.js";
    let mut loader = MemoryLoader::new();
    loader.insert(page, "<script src=\"/app.js\"></script>");
    loader.insert(script_url, script);
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
fn latin1_accept_value_uses_bytestring_length_at_128_byte_boundary() {
    let endpoint = "http://api.test/data";
    let value = "é".repeat(128);
    let script = format!(
        "fetch(\"{endpoint}\", {{ headers: {{ Accept: \"{value}\" }} }});"
    );
    let (mut browser, transport) = browser_for(&script, endpoint);

    assert_eq!(browser.tick().requests_sent, 1);
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method.as_str(), "GET");
    assert_eq!(requests[0].headers.get("accept").as_deref(), Some(value.as_str()));
}

#[test]
fn latin1_accept_value_over_128_bytes_requires_preflight() {
    let endpoint = "http://api.test/data";
    let value = "é".repeat(129);
    let script = format!(
        "fetch(\"{endpoint}\", {{ headers: {{ Accept: \"{value}\" }} }});"
    );
    let (mut browser, transport) = browser_for(&script, endpoint);

    assert_eq!(browser.tick().requests_sent, 1);
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method.as_str(), "OPTIONS");
    assert_eq!(
        requests[0]
            .headers
            .get("access-control-request-headers")
            .as_deref(),
        Some("accept")
    );
}
