use std::rc::Rc;

use browser_engine::browser::Browser;
use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Url};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn response(endpoint: &str, allow_headers: Option<&str>) -> FetchResponse {
    let mut response =
        FetchResponse::synthetic(url(endpoint), 200, Some("text/plain"), b"ok".to_vec());
    response
        .headers
        .append_raw("access-control-allow-origin", "*");
    if let Some(value) = allow_headers {
        response
            .headers
            .append_raw("access-control-allow-headers", value);
    }
    response
}

fn browser_for(script: &str, endpoint: &str, response: FetchResponse) -> (Browser, Rc<ManualNetwork>) {
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

fn repeated_accept_script(extra: &str) -> String {
    let value = "a".repeat(128);
    let mut script = String::from("const h = new Headers();");
    for _ in 0..8 {
        script.push_str(&format!("h.append(\"Accept\", \"{value}\");"));
    }
    script.push_str(extra);
    script.push_str(
        r#"fetch("http://api.test/data", { headers: h })
             .then(function () { console.log("ok"); })
             .catch(function () { console.log("blocked"); });"#,
    );
    script
}

#[test]
fn exactly_1024_safelisted_value_bytes_still_dispatch_directly() {
    let endpoint = "http://api.test/data";
    let script = repeated_accept_script("");
    let (mut browser, transport) = browser_for(&script, endpoint, response(endpoint, None));

    let tick = browser.tick();
    assert_eq!(tick.requests_sent, 1);
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method.as_str(), "GET");
    assert!(requests[0]
        .headers
        .get("access-control-request-headers")
        .is_none());
}

#[test]
fn byte_1025_promotes_all_safelisted_names_into_preflight() {
    let endpoint = "http://api.test/data";
    let script = repeated_accept_script("h.append(\"Content-Language\", \"e\");");
    let (mut browser, transport) = browser_for(
        &script,
        endpoint,
        response(endpoint, Some("accept, content-language")),
    );

    let tick = browser.tick();
    assert_eq!(tick.requests_sent, 1);
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method.as_str(), "OPTIONS");
    assert_eq!(
        requests[0]
            .headers
            .get("access-control-request-headers")
            .as_deref(),
        Some("accept,content-language")
    );

    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].method.as_str(), "GET");
}
