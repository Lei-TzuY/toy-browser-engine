use std::rc::Rc;
use std::time::Duration;

use browser_engine::browser::Browser;
use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Url};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn methods(transport: &ManualNetwork) -> Vec<String> {
    transport
        .requests()
        .iter()
        .map(|request| request.method.as_str().to_string())
        .collect()
}

#[test]
fn zero_max_age_remains_immediately_expired_after_the_session_clock_has_advanced() {
    let page = "http://page.test/index.html";
    let endpoint = "http://api.test/data";
    let script = r#"
        fetch("http://api.test/data", {
            method: "PUT",
            headers: { "X-Token": "one" },
            body: "a"
        }).then(function () {
            return fetch("http://api.test/data", {
                method: "PUT",
                headers: { "X-Token": "two" },
                body: "b"
            });
        }).then(function () { console.log("done"); });
    "#;

    let mut loader = MemoryLoader::new();
    loader.insert(page, format!("<script>{script}</script>"));

    let mut preflight = FetchResponse::synthetic(
        url(endpoint),
        200,
        Some("text/plain"),
        b"ok".to_vec(),
    );
    preflight
        .headers
        .append_raw("access-control-allow-origin", "*");
    preflight
        .headers
        .append_raw("access-control-allow-methods", "PUT");
    preflight
        .headers
        .append_raw("access-control-allow-headers", "x-token");
    preflight
        .headers
        .append_raw("access-control-max-age", "0");

    let transport = Rc::new(ManualNetwork::new());
    transport.respond(endpoint, preflight);
    let mut browser = Browser::open_with_network(
        Box::new(loader),
        transport.clone(),
        &url(page),
        Rc::new(ManualClock::new()),
    )
    .expect("page loads");

    // Exercise max-age zero when the effective session time is non-zero. The
    // cache must not turn that absolute timestamp into a reusable permission.
    browser.advance_time(Duration::from_millis(1_000));

    for _ in 0..12 {
        browser.tick();
        transport.complete_all();
    }
    browser.tick();

    assert_eq!(methods(&transport), vec!["OPTIONS", "PUT", "OPTIONS", "PUT"]);
    assert_eq!(browser.document().runtime.console, vec!["done"]);
}
