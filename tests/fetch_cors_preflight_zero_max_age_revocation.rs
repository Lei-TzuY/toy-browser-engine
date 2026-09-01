use std::rc::Rc;

use browser_engine::browser::Browser;
use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Url};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn response(endpoint: &str, max_age: &str, allowed_header: &str) -> FetchResponse {
    let mut response = FetchResponse::synthetic(
        url(endpoint),
        200,
        Some("text/plain"),
        b"ok".to_vec(),
    );
    response
        .headers
        .append_raw("access-control-allow-origin", "*");
    response
        .headers
        .append_raw("access-control-allow-methods", "PUT");
    response
        .headers
        .append_raw("access-control-allow-headers", allowed_header);
    response
        .headers
        .append_raw("access-control-max-age", max_age);
    response
}

fn methods(transport: &ManualNetwork) -> Vec<String> {
    transport
        .requests()
        .iter()
        .map(|request| request.method.as_str().to_string())
        .collect()
}

#[test]
fn zero_max_age_preflight_revokes_an_older_positive_cache_entry() {
    let page = "http://page.test/index.html";
    let endpoint = "http://api.test/data";
    let script = r#"
        fetch("http://api.test/data", { method: "PUT", headers: { "X-Old": "one" }, body: "a" })
          .then(function () {
              return fetch("http://api.test/data", { method: "PUT", headers: { "X-New": "two" }, body: "b" });
          })
          .then(function () {
              return fetch("http://api.test/data", { method: "PUT", headers: { "X-Old": "three" }, body: "c" });
          })
          .then(function () { console.log("done"); })
          .catch(function () { console.log("blocked"); });
    "#;

    let mut loader = MemoryLoader::new();
    loader.insert(page, format!("<script>{script}</script>"));
    let transport = Rc::new(ManualNetwork::new());
    transport.respond(endpoint, response(endpoint, "60", "x-old"));
    let mut browser = Browser::open_with_network(
        Box::new(loader),
        transport.clone(),
        &url(page),
        Rc::new(ManualClock::new()),
    )
    .expect("page loads");

    // First request: cache x-old for 60 seconds.
    browser.tick();
    assert_eq!(transport.complete_all(), 1); // OPTIONS
    browser.tick();
    assert_eq!(transport.complete_all(), 1); // PUT

    // The second request uses a different header, forcing another preflight.
    // Its successful response explicitly gives the cache a zero lifetime.
    transport.respond(endpoint, response(endpoint, "0", "x-new"));
    browser.tick();
    assert_eq!(methods(&transport), vec!["OPTIONS", "PUT", "OPTIONS"]);
    assert_eq!(transport.complete_all(), 1); // second OPTIONS
    browser.tick();
    assert_eq!(transport.complete_all(), 1); // second PUT

    // Before the chained third request starts, make its own preflight succeed.
    // The important assertion is that x-old no longer bypasses OPTIONS merely
    // because it had been cached by the first response.
    transport.respond(endpoint, response(endpoint, "0", "x-old"));
    browser.tick();
    assert_eq!(
        methods(&transport),
        vec!["OPTIONS", "PUT", "OPTIONS", "PUT", "OPTIONS"]
    );
    assert_eq!(transport.complete_all(), 1); // third OPTIONS
    browser.tick();
    assert_eq!(transport.complete_all(), 1); // third PUT
    browser.tick();

    assert_eq!(
        methods(&transport),
        vec!["OPTIONS", "PUT", "OPTIONS", "PUT", "OPTIONS", "PUT"]
    );
    assert_eq!(browser.document().runtime.console, vec!["done"]);
}
