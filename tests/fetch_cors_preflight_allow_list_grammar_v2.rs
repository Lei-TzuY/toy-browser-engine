use std::rc::Rc;

use browser_engine::browser::Browser;
use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Url};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn preflight_response(
    endpoint: &str,
    allow_methods: Option<&str>,
    allow_headers: Option<&str>,
) -> FetchResponse {
    let mut response =
        FetchResponse::synthetic(url(endpoint), 200, Some("text/plain"), b"ok".to_vec());
    response
        .headers
        .append_raw("access-control-allow-origin", "*");
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

fn finish_preflight(browser: &mut Browser, transport: &Rc<ManualNetwork>) {
    assert_eq!(transport.complete_all(), 1);
    browser.tick();
}

#[test]
fn malformed_allow_methods_member_rejects_the_entire_preflight_list() {
    let endpoint = "http://api.test/data";
    let (mut browser, transport) = browser_for(
        r#"fetch("http://api.test/data", { method: "PUT" })
             .then(function () { console.log("ok"); })
             .catch(function () { console.log("blocked"); });"#,
        endpoint,
        preflight_response(endpoint, Some("PUT, bad method"), None),
    );

    let first = browser.tick();
    assert_eq!(first.requests_sent, 1);
    assert_eq!(transport.requests()[0].method.as_str(), "OPTIONS");

    finish_preflight(&mut browser, &transport);
    assert_eq!(
        transport.requests().len(),
        1,
        "malformed list must not dispatch PUT"
    );
    assert_eq!(browser.document().runtime.console, vec!["blocked"]);
}

#[test]
fn malformed_allow_headers_member_rejects_even_when_requested_name_is_present() {
    let endpoint = "http://api.test/data";
    let (mut browser, transport) = browser_for(
        r#"fetch("http://api.test/data", { headers: { "X-Token": "secret" } })
             .then(function () { console.log("ok"); })
             .catch(function () { console.log("blocked"); });"#,
        endpoint,
        preflight_response(endpoint, None, Some("x-token, bad/name")),
    );

    let first = browser.tick();
    assert_eq!(first.requests_sent, 1);
    assert_eq!(transport.requests()[0].method.as_str(), "OPTIONS");

    finish_preflight(&mut browser, &transport);
    assert_eq!(
        transport.requests().len(),
        1,
        "malformed list must not dispatch GET"
    );
    assert_eq!(browser.document().runtime.console, vec!["blocked"]);
}

#[test]
fn valid_token_lists_with_empty_members_still_authorize_the_actual_request() {
    let endpoint = "http://api.test/data";
    let (mut browser, transport) = browser_for(
        r#"fetch("http://api.test/data", {
               method: "PUT",
               headers: { "X-Token": "secret" }
             }).then(function () { console.log("ok"); })
               .catch(function () { console.log("blocked"); });"#,
        endpoint,
        preflight_response(endpoint, Some("PUT, ,"), Some(", x-token, ")),
    );

    let first = browser.tick();
    assert_eq!(first.requests_sent, 1);
    assert_eq!(transport.requests()[0].method.as_str(), "OPTIONS");

    finish_preflight(&mut browser, &transport);
    assert_eq!(transport.requests().len(), 2);
    assert_eq!(transport.requests()[1].method.as_str(), "PUT");
}
