use std::rc::Rc;

use browser_engine::browser::Browser;
use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Url};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn response(endpoint: &str, max_age: &str, methods: &str, headers: &str) -> FetchResponse {
    let mut r = FetchResponse::synthetic(url(endpoint), 200, Some("text/plain"), b"ok".to_vec());
    r.headers.append_raw("access-control-allow-origin", "*");
    r.headers
        .append_raw("access-control-allow-methods", methods);
    r.headers
        .append_raw("access-control-allow-headers", headers);
    r.headers.append_raw("access-control-max-age", max_age);
    r
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

fn drive(browser: &mut Browser, transport: &Rc<ManualNetwork>) {
    for _ in 0..12 {
        browser.tick();
        transport.complete_all();
    }
    browser.tick();
}

fn methods(transport: &ManualNetwork) -> Vec<&'static str> {
    transport
        .requests()
        .iter()
        .map(|request| match request.method.as_str() {
            "OPTIONS" => "OPTIONS",
            "PUT" => "PUT",
            "PATCH" => "PATCH",
            other => panic!("unexpected method {other}"),
        })
        .collect()
}

#[test]
fn positive_max_age_reuses_preflight_for_an_identical_request() {
    let endpoint = "http://api.test/data";
    let script = r#"
        fetch("http://api.test/data", { method: "PUT", headers: { "X-Token": "one" }, body: "a" })
          .then(function () { return fetch("http://api.test/data", { method: "PUT", headers: { "X-Token": "two" }, body: "b" }); })
          .then(function () { console.log("done"); }).catch(function () { console.log("blocked"); });
    "#;
    let (mut browser, transport) =
        browser_for(script, endpoint, response(endpoint, "60", "PUT", "x-token"));
    drive(&mut browser, &transport);
    assert_eq!(methods(&transport), vec!["OPTIONS", "PUT", "PUT"]);
    assert_eq!(browser.document().runtime.console, vec!["done"]);
}

#[test]
fn cache_reuses_all_method_and_header_permissions_granted_by_the_response() {
    let endpoint = "http://api.test/data";
    let script = r#"
        fetch("http://api.test/data", { method: "PUT", headers: { "X-Token": "one" }, body: "a" })
          .then(function () { return fetch("http://api.test/data", { method: "PATCH", headers: { "X-Other": "two" }, body: "b" }); })
          .then(function () { console.log("done"); });
    "#;
    let (mut browser, transport) = browser_for(
        script,
        endpoint,
        response(endpoint, "60", "PUT, PATCH", "x-token, x-other"),
    );
    drive(&mut browser, &transport);
    assert_eq!(methods(&transport), vec!["OPTIONS", "PUT", "PATCH"]);
    assert_eq!(browser.document().runtime.console, vec!["done"]);
}

#[test]
fn zero_max_age_forces_the_next_request_to_preflight_again() {
    let endpoint = "http://api.test/data";
    let script = r#"
        fetch("http://api.test/data", { method: "PUT", headers: { "X-Token": "one" }, body: "a" })
          .then(function () { return fetch("http://api.test/data", { method: "PUT", headers: { "X-Token": "two" }, body: "b" }); })
          .then(function () { console.log("done"); });
    "#;
    let (mut browser, transport) =
        browser_for(script, endpoint, response(endpoint, "0", "PUT", "x-token"));
    drive(&mut browser, &transport);
    assert_eq!(
        methods(&transport),
        vec!["OPTIONS", "PUT", "OPTIONS", "PUT"]
    );
    assert_eq!(browser.document().runtime.console, vec!["done"]);
}

#[test]
fn authorization_is_not_satisfied_by_the_allow_headers_wildcard() {
    let endpoint = "http://api.test/data";
    let script = r#"
        fetch("http://api.test/data", { method: "PUT", headers: { "X-Token": "one" }, body: "a" })
          .then(function () { return fetch("http://api.test/data", { method: "PUT", headers: { "Authorization": "Bearer x" }, body: "b" }); })
          .catch(function () { console.log("blocked"); });
    "#;
    let (mut browser, transport) =
        browser_for(script, endpoint, response(endpoint, "60", "*", "*"));
    drive(&mut browser, &transport);
    assert_eq!(methods(&transport), vec!["OPTIONS", "PUT", "OPTIONS"]);
    assert_eq!(browser.document().runtime.console, vec!["blocked"]);
}
