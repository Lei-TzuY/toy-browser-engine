use std::rc::Rc;

use browser_engine::browser::Browser;
use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Method, Url};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn response(endpoint: &str) -> FetchResponse {
    let mut response = FetchResponse::synthetic(
        url(endpoint),
        201,
        Some("text/plain"),
        b"secret body".to_vec(),
    );
    response.headers.append_raw("x-secret", "classified");
    response
}

fn browser_for(page: &str, script: &str, endpoint: &str, response: FetchResponse) -> (Browser, Rc<ManualNetwork>) {
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

#[test]
fn no_cors_mode_is_visible_cloned_and_produces_an_opaque_response() {
    let page = "http://page.test/index.html";
    let endpoint = "http://api.test/data";
    let (mut browser, transport) = browser_for(
        page,
        r#"
            var original = new Request("http://api.test/data", { mode: "no-cors" });
            var clone = new Request(original);
            console.log(original.mode);
            console.log(clone.mode);
            fetch(clone).then(function (response) { console.log(response.type); });
        "#,
        endpoint,
        response(endpoint),
    );

    assert_eq!(browser.document().runtime.console, vec!["no-cors", "no-cors"]);
    assert_eq!(browser.tick().requests_sent, 1);
    let sent = transport.requests();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].headers.get("origin").is_none());

    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(
        browser.document().runtime.console,
        vec!["no-cors", "no-cors", "opaque"]
    );
}

#[test]
fn opaque_response_hides_status_url_headers_and_body() {
    let page = "http://page.test/index.html";
    let endpoint = "http://api.test/data";
    let (mut browser, transport) = browser_for(
        page,
        r#"
            fetch("http://api.test/data", { mode: "no-cors" }).then(function (response) {
                console.log(response.type === "opaque" ? "type-ok" : "type-leak");
                console.log(response.status === 0 ? "status-ok" : "status-leak");
                console.log(response.statusText === "" ? "status-text-ok" : "status-text-leak");
                console.log(response.ok === false ? "ok-filtered" : "ok-leak");
                console.log(response.url === "" ? "url-ok" : "url-leak");
                console.log(response.redirected === false ? "redirect-ok" : "redirect-leak");
                console.log(response.headers.get("x-secret") === null ? "headers-ok" : "headers-leak");
                console.log(response.bodyUsed === false ? "body-unused" : "body-used");
                return response.text().then(function (text) {
                    console.log(text === "" ? "body-ok" : "body-leak");
                    console.log(response.bodyUsed === false ? "body-still-unused" : "body-used-after-read");
                });
            });
        "#,
        endpoint,
        response(endpoint),
    );

    assert_eq!(browser.tick().requests_sent, 1);
    assert_eq!(transport.complete_all(), 1);
    browser.tick();

    assert_eq!(
        browser.document().runtime.console,
        vec![
            "type-ok",
            "status-ok",
            "status-text-ok",
            "ok-filtered",
            "url-ok",
            "redirect-ok",
            "headers-ok",
            "body-unused",
            "body-ok",
            "body-still-unused",
        ]
    );
}

#[test]
fn no_cors_drops_unsafe_headers_without_preflighting() {
    let page = "http://page.test/index.html";
    let endpoint = "http://api.test/data";
    let (mut browser, transport) = browser_for(
        page,
        r#"
            fetch("http://api.test/data", {
                mode: "no-cors",
                method: "POST",
                headers: {
                    "X-Token": "secret",
                    "Content-Type": "application/json"
                },
                body: "payload"
            }).then(function (response) { console.log(response.type); });
        "#,
        endpoint,
        response(endpoint),
    );

    assert_eq!(browser.tick().requests_sent, 1);
    let sent = transport.requests();
    assert_eq!(sent.len(), 1, "no OPTIONS preflight is created");
    assert_eq!(sent[0].method, Method::Post);
    assert!(sent[0].headers.get("x-token").is_none());
    assert!(sent[0].headers.get("content-type").is_none());
    assert!(sent[0].headers.get("origin").is_none());

    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(browser.document().runtime.console, vec!["opaque"]);
}

#[test]
fn no_cors_rejects_a_non_safelisted_method_before_transport() {
    let page = "http://page.test/index.html";
    let endpoint = "http://api.test/data";
    let (browser, transport) = browser_for(
        page,
        r#"
            fetch("http://api.test/data", { mode: "no-cors", method: "PUT" })
                .then(function () { console.log("unexpected"); })
                .catch(function () { console.log("blocked"); });
        "#,
        endpoint,
        response(endpoint),
    );

    assert!(transport.requests().is_empty());
    assert_eq!(browser.document().runtime.console, vec!["blocked"]);
}

#[test]
fn same_origin_no_cors_response_remains_basic_and_readable() {
    let page = "http://page.test/index.html";
    let endpoint = "http://page.test/data";
    let (mut browser, transport) = browser_for(
        page,
        r#"
            fetch("/data", { mode: "no-cors" }).then(function (response) {
                console.log(response.type);
                return response.text().then(function (text) { console.log(text); });
            });
        "#,
        endpoint,
        response(endpoint),
    );

    assert_eq!(browser.tick().requests_sent, 1);
    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(browser.document().runtime.console, vec!["basic", "secret body"]);
}

#[test]
fn credentialed_no_cors_updates_cookie_state_but_stays_opaque() {
    let page = "http://page.test:8000/index.html";
    let endpoint = "http://page.test:9000/data";
    let mut wire = response(endpoint);
    wire.headers.append_raw("set-cookie", "server=new; Path=/");

    let (mut browser, transport) = browser_for(
        page,
        r#"
            fetch("http://page.test:9000/data", {
                mode: "no-cors",
                credentials: "include"
            }).then(function (response) {
                console.log(response.type);
                console.log(response.headers.get("set-cookie") === null ? "cookie-hidden" : "cookie-leak");
            });
        "#,
        endpoint,
        wire,
    );

    let jar = browser.cookie_jar();
    assert!(jar.borrow_mut().store_set_cookie(
        "session=old; Path=/",
        &url("http://page.test:9000/"),
        0,
    ));

    assert_eq!(browser.tick().requests_sent, 1);
    let sent = transport.requests();
    assert_eq!(sent[0].headers.get("cookie").as_deref(), Some("session=old"));
    assert!(sent[0].headers.get("origin").is_none());

    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(browser.document().runtime.console, vec!["opaque", "cookie-hidden"]);

    let stored = jar
        .borrow()
        .get_http_cookie_header(&url(endpoint), 0)
        .unwrap_or_default();
    assert!(stored.contains("session=old"), "{stored}");
    assert!(stored.contains("server=new"), "{stored}");
}
