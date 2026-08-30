use std::rc::Rc;

use browser_engine::browser::Browser;
use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Url};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn browser_for(
    page: &str,
    script: &str,
    endpoint: &str,
    response: FetchResponse,
) -> (Browser, Rc<ManualNetwork>) {
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

fn complete_fetch(browser: &mut Browser, transport: &ManualNetwork) {
    let sent = browser.tick();
    assert_eq!(sent.requests_sent, 1);
    assert_eq!(transport.complete_all(), 1);
    let completed = browser.tick();
    assert_eq!(completed.network_completions, 1);
}

fn cors_response(endpoint: &str, credentialed: bool) -> FetchResponse {
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
    if credentialed {
        response
            .headers
            .append_raw("access-control-allow-credentials", "true");
    }
    response
}

#[test]
fn cross_origin_response_exposes_only_cors_safelisted_headers_by_default() {
    let endpoint = "http://api.test/data";
    let mut response = cors_response(endpoint, false);
    response.headers.append_raw("cache-control", "max-age=60");
    response.headers.append_raw("content-language", "en");
    response
        .headers
        .append_raw("expires", "Wed, 21 Oct 2026 07:28:00 GMT");
    response
        .headers
        .append_raw("last-modified", "Tue, 20 Oct 2026 07:28:00 GMT");
    response.headers.append_raw("pragma", "no-cache");
    response.headers.append_raw("x-secret", "hidden");

    let script = r#"fetch("http://api.test/data").then(function (r) {
        console.log(r.headers.has("cache-control") ? "cache" : "no-cache");
        console.log(r.headers.has("content-language") ? "language" : "no-language");
        console.log(r.headers.has("content-length") ? "length" : "no-length");
        console.log(r.headers.has("content-type") ? "type" : "no-type");
        console.log(r.headers.has("expires") ? "expires" : "no-expires");
        console.log(r.headers.has("last-modified") ? "modified" : "no-modified");
        console.log(r.headers.has("pragma") ? "pragma" : "no-pragma");
        console.log(r.headers.has("x-secret") ? "secret" : "hidden");
        console.log(r.headers.has("access-control-allow-origin") ? "acao" : "no-acao");
    });"#;
    let (mut browser, transport) =
        browser_for("http://page.test/index.html", script, endpoint, response);

    complete_fetch(&mut browser, &transport);
    assert_eq!(
        browser.document().runtime.console,
        vec![
            "cache", "language", "length", "type", "expires", "modified", "pragma", "hidden",
            "no-acao"
        ]
    );
}

#[test]
fn expose_headers_adds_named_headers_case_insensitively() {
    let endpoint = "http://api.test/data";
    let mut response = cors_response(endpoint, false);
    response
        .headers
        .append_raw("access-control-expose-headers", "X-Request-ID, x-trace");
    response.headers.append_raw("x-request-id", "req-1");
    response.headers.append_raw("X-Trace", "trace-1");
    response.headers.append_raw("x-private", "private");

    let script = r#"fetch("http://api.test/data").then(function (r) {
        console.log(r.headers.get("x-request-id"));
        console.log(r.headers.get("x-trace"));
        console.log(r.headers.has("x-private") ? "private" : "hidden");
        console.log(r.headers.has("access-control-expose-headers") ? "aceh" : "no-aceh");
    });"#;
    let (mut browser, transport) =
        browser_for("http://page.test/index.html", script, endpoint, response);

    complete_fetch(&mut browser, &transport);
    assert_eq!(
        browser.document().runtime.console,
        vec!["req-1", "trace-1", "hidden", "no-aceh"]
    );
}

#[test]
fn noncredentialed_expose_headers_wildcard_exposes_all_non_forbidden_headers() {
    let endpoint = "http://api.test/data";
    let mut response = cors_response(endpoint, false);
    response
        .headers
        .append_raw("access-control-expose-headers", "*");
    response.headers.append_raw("x-secret", "visible");
    response.headers.append_raw("set-cookie2", "legacy=secret");

    let script = r#"fetch("http://api.test/data").then(function (r) {
        console.log(r.headers.get("x-secret"));
        console.log(r.headers.has("access-control-allow-origin") ? "acao" : "no-acao");
        console.log(r.headers.has("set-cookie2") ? "cookie" : "no-cookie");
    });"#;
    let (mut browser, transport) =
        browser_for("http://page.test/index.html", script, endpoint, response);

    complete_fetch(&mut browser, &transport);
    assert_eq!(
        browser.document().runtime.console,
        vec!["visible", "acao", "no-cookie"]
    );
}

#[test]
fn credentialed_expose_headers_wildcard_is_literal_not_a_wildcard() {
    let endpoint = "http://api.test/data";
    let mut response = cors_response(endpoint, true);
    response
        .headers
        .append_raw("access-control-expose-headers", "*");
    response.headers.append_raw("x-secret", "hidden");
    response.headers.append_raw("*", "literal-star");

    let script = r#"fetch("http://api.test/data", { credentials: "include" }).then(function (r) {
        console.log(r.headers.has("x-secret") ? "secret" : "hidden");
        console.log(r.headers.get("*"));
    });"#;
    let (mut browser, transport) =
        browser_for("http://page.test/index.html", script, endpoint, response);

    complete_fetch(&mut browser, &transport);
    assert_eq!(
        browser.document().runtime.console,
        vec!["hidden", "literal-star"]
    );
}

#[test]
fn credentialed_response_can_explicitly_expose_a_custom_header() {
    let endpoint = "http://api.test/data";
    let mut response = cors_response(endpoint, true);
    response
        .headers
        .append_raw("access-control-expose-headers", "X-Secret");
    response.headers.append_raw("x-secret", "visible");

    let script = r#"fetch("http://api.test/data", { credentials: "include" }).then(function (r) {
        console.log(r.headers.get("x-secret"));
    });"#;
    let (mut browser, transport) =
        browser_for("http://page.test/index.html", script, endpoint, response);

    complete_fetch(&mut browser, &transport);
    assert_eq!(browser.document().runtime.console, vec!["visible"]);
}

#[test]
fn same_origin_fetch_keeps_ordinary_response_headers_visible() {
    let endpoint = "http://page.test/data";
    let mut response =
        FetchResponse::synthetic(url(endpoint), 200, Some("text/plain"), b"ok".to_vec());
    response.headers.append_raw("x-internal", "same-origin");

    let script = r#"fetch("http://page.test/data").then(function (r) {
        console.log(r.headers.get("x-internal"));
    });"#;
    let (mut browser, transport) =
        browser_for("http://page.test/index.html", script, endpoint, response);

    complete_fetch(&mut browser, &transport);
    assert_eq!(browser.document().runtime.console, vec!["same-origin"]);
}
