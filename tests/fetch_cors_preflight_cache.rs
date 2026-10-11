use std::rc::Rc;
use std::time::Duration;

use browser_engine::browser::Browser;
use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Url};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn response(
    endpoint: &str,
    max_age: &str,
    methods: &str,
    headers: &str,
    credentialed: bool,
) -> FetchResponse {
    let mut response =
        FetchResponse::synthetic(url(endpoint), 200, Some("text/plain"), b"ok".to_vec());
    response.headers.append_raw(
        "access-control-allow-origin",
        if credentialed {
            "http://page.test"
        } else {
            "*"
        },
    );
    response
        .headers
        .append_raw("access-control-allow-methods", methods);
    response
        .headers
        .append_raw("access-control-allow-headers", headers);
    response
        .headers
        .append_raw("access-control-max-age", max_age);
    if credentialed {
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

fn drive(browser: &mut Browser, transport: &Rc<ManualNetwork>) {
    for _ in 0..12 {
        browser.tick();
        transport.complete_all();
    }
    browser.tick();
}

fn methods(transport: &ManualNetwork) -> Vec<String> {
    transport
        .requests()
        .iter()
        .map(|request| request.method.as_str().to_string())
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
    let (mut browser, transport) = browser_for(
        script,
        endpoint,
        response(endpoint, "60", "PUT", "x-token", false),
    );

    drive(&mut browser, &transport);

    assert_eq!(methods(&transport), vec!["OPTIONS", "PUT", "PUT"]);
    assert_eq!(browser.document().runtime.console, vec!["done"]);
}

#[test]
fn cache_reuses_all_permissions_granted_by_the_preflight_response() {
    let endpoint = "http://api.test/data";
    let script = r#"
        fetch("http://api.test/data", { method: "PUT", headers: { "X-Token": "one" }, body: "a" })
          .then(function () { return fetch("http://api.test/data", { method: "PATCH", headers: { "X-Other": "two" }, body: "b" }); })
          .then(function () { console.log("done"); });
    "#;
    let (mut browser, transport) = browser_for(
        script,
        endpoint,
        response(endpoint, "60", "PUT, PATCH", "x-token, x-other", false),
    );

    drive(&mut browser, &transport);

    assert_eq!(methods(&transport), vec!["OPTIONS", "PUT", "PATCH"]);
    assert_eq!(browser.document().runtime.console, vec!["done"]);
}

#[test]
fn max_age_zero_forces_the_next_request_to_preflight_again() {
    let endpoint = "http://api.test/data";
    let script = r#"
        fetch("http://api.test/data", { method: "PUT", headers: { "X-Token": "one" }, body: "a" })
          .then(function () { return fetch("http://api.test/data", { method: "PUT", headers: { "X-Token": "two" }, body: "b" }); })
          .then(function () { console.log("done"); });
    "#;
    let (mut browser, transport) = browser_for(
        script,
        endpoint,
        response(endpoint, "0", "PUT", "x-token", false),
    );

    drive(&mut browser, &transport);

    assert_eq!(
        methods(&transport),
        vec!["OPTIONS", "PUT", "OPTIONS", "PUT"]
    );
}

#[test]
fn expired_entry_is_not_reused_after_the_session_clock_advances() {
    let endpoint = "http://api.test/data";
    let script = r#"
        fetch("http://api.test/data", { method: "PUT", headers: { "X-Token": "one" }, body: "a" })
          .then(function () {
              setTimeout(function () {
                  fetch("http://api.test/data", { method: "PUT", headers: { "X-Token": "two" }, body: "b" });
              }, 1100);
          });
    "#;
    let (mut browser, transport) = browser_for(
        script,
        endpoint,
        response(endpoint, "1", "PUT", "x-token", false),
    );

    browser.tick();
    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(methods(&transport), vec!["OPTIONS", "PUT"]);

    browser.advance_time(Duration::from_millis(1100));
    browser.tick();

    assert_eq!(methods(&transport), vec!["OPTIONS", "PUT", "OPTIONS"]);
}

#[test]
fn enormous_valid_max_age_clamps_instead_of_falling_back_to_five_seconds() {
    let endpoint = "http://api.test/data";
    let script = r#"
        fetch("http://api.test/data", { method: "PUT", headers: { "X-Token": "one" }, body: "a" })
          .then(function () {
              setTimeout(function () {
                  fetch("http://api.test/data", { method: "PUT", headers: { "X-Token": "two" }, body: "b" });
              }, 6000);
          });
    "#;
    let huge = "999999999999999999999999999999999999999999999999999999999999";
    let (mut browser, transport) = browser_for(
        script,
        endpoint,
        response(endpoint, huge, "PUT", "x-token", false),
    );

    browser.tick();
    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(methods(&transport), vec!["OPTIONS", "PUT"]);

    browser.advance_time(Duration::from_millis(6000));
    browser.tick();

    assert_eq!(methods(&transport), vec!["OPTIONS", "PUT", "PUT"]);
}

#[test]
fn non_abnf_max_age_falls_back_to_five_seconds() {
    let endpoint = "http://api.test/data";
    let script = r#"
        fetch("http://api.test/data", { method: "PUT", headers: { "X-Token": "one" }, body: "a" })
          .then(function () {
              setTimeout(function () {
                  fetch("http://api.test/data", { method: "PUT", headers: { "X-Token": "two" }, body: "b" });
              }, 5500);
          });
    "#;
    let (mut browser, transport) = browser_for(
        script,
        endpoint,
        response(endpoint, "+60", "PUT", "x-token", false),
    );

    browser.tick();
    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(transport.complete_all(), 1);
    browser.tick();

    browser.advance_time(Duration::from_millis(5500));
    browser.tick();

    assert_eq!(methods(&transport), vec!["OPTIONS", "PUT", "OPTIONS"]);
}

#[test]
fn duplicate_max_age_fields_fall_back_to_five_seconds() {
    let endpoint = "http://api.test/data";
    let script = r#"
        fetch("http://api.test/data", { method: "PUT", headers: { "X-Token": "one" }, body: "a" })
          .then(function () {
              setTimeout(function () {
                  fetch("http://api.test/data", { method: "PUT", headers: { "X-Token": "two" }, body: "b" });
              }, 5500);
          });
    "#;
    let mut preflight = response(endpoint, "60", "PUT", "x-token", false);
    preflight.headers.append_raw("access-control-max-age", "60");
    let (mut browser, transport) = browser_for(script, endpoint, preflight);

    browser.tick();
    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(transport.complete_all(), 1);
    browser.tick();

    browser.advance_time(Duration::from_millis(5500));
    browser.tick();

    assert_eq!(methods(&transport), vec!["OPTIONS", "PUT", "OPTIONS"]);
}

#[test]
fn noncredentialed_entry_does_not_authorize_a_later_credentialed_request() {
    let endpoint = "http://api.test/data";
    let script = r#"
        fetch("http://api.test/data", { method: "PUT", headers: { "X-Token": "one" }, body: "a", credentials: "omit" })
          .then(function () { return fetch("http://api.test/data", { method: "PUT", headers: { "X-Token": "two" }, body: "b", credentials: "include" }); });
    "#;
    let (mut browser, transport) = browser_for(
        script,
        endpoint,
        response(endpoint, "60", "PUT", "x-token", true),
    );

    drive(&mut browser, &transport);

    assert_eq!(
        methods(&transport),
        vec!["OPTIONS", "PUT", "OPTIONS", "PUT"]
    );
}

#[test]
fn authorization_is_not_satisfied_by_allow_headers_wildcard() {
    let endpoint = "http://api.test/data";
    let script = r#"
        fetch("http://api.test/data", { method: "PUT", headers: { "X-Token": "one" }, body: "a" })
          .then(function () { return fetch("http://api.test/data", { method: "PUT", headers: { "Authorization": "Bearer x" }, body: "b" }); });
    "#;
    let (mut browser, transport) =
        browser_for(script, endpoint, response(endpoint, "60", "*", "*", false));

    drive(&mut browser, &transport);

    assert_eq!(methods(&transport), vec!["OPTIONS", "PUT", "OPTIONS"]);
}

#[test]
fn actual_network_failure_invalidates_cached_preflight_permissions() {
    let endpoint = "http://api.test/data";
    let script = r#"
        fetch("http://api.test/data", { method: "PUT", headers: { "X-Token": "one" }, body: "a" })
          .then(function () { return fetch("http://api.test/data", { method: "PUT", headers: { "X-Token": "two" }, body: "b" }); })
          .catch(function () { return fetch("http://api.test/data", { method: "PUT", headers: { "X-Token": "three" }, body: "c" }); });
    "#;
    let (mut browser, transport) = browser_for(
        script,
        endpoint,
        response(endpoint, "60", "PUT", "x-token", false),
    );

    browser.tick();
    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(transport.complete_all(), 1);

    transport.fail(endpoint, browser_engine::net::FetchError::Io("boom".into()));
    browser.tick();
    assert_eq!(methods(&transport), vec!["OPTIONS", "PUT", "PUT"]);
    assert_eq!(transport.complete_all(), 1);

    browser.tick();
    assert_eq!(
        methods(&transport),
        vec!["OPTIONS", "PUT", "PUT", "OPTIONS"]
    );
}
