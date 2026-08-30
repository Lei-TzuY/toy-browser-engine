use std::rc::Rc;

use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Url};
use browser_engine::Browser;

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn browser_for(script: &str, transport: Rc<ManualNetwork>) -> Browser {
    let mut loader = MemoryLoader::new();
    loader.insert(
        "http://page.test/index.html",
        format!("<script>{script}</script>"),
    );
    Browser::open_with_single_hop_network(
        Box::new(loader),
        transport,
        &url("http://page.test/index.html"),
        Rc::new(ManualClock::new()),
    )
    .expect("browser opens")
}

fn redirect(endpoint: &str, location: &str, allow_origin: Option<&str>) -> FetchResponse {
    let mut response = FetchResponse::synthetic(
        url(endpoint),
        302,
        Some("text/plain"),
        b"redirect".to_vec(),
    );
    response.headers.insert_raw("location", location);
    if let Some(origin) = allow_origin {
        response
            .headers
            .insert_raw("access-control-allow-origin", origin);
    }
    response
}

fn ok(endpoint: &str, body: &str, allow_origin: Option<&str>) -> FetchResponse {
    let mut response = FetchResponse::synthetic(
        url(endpoint),
        200,
        Some("text/plain"),
        body.as_bytes().to_vec(),
    );
    if let Some(origin) = allow_origin {
        response
            .headers
            .insert_raw("access-control-allow-origin", origin);
    }
    response
}

#[test]
fn same_origin_request_can_follow_simple_cross_origin_cors_redirect() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    transport.respond(
        "http://page.test/start",
        redirect("http://page.test/start", "http://api.test/final", None),
    );
    transport.respond(
        "http://api.test/final",
        ok("http://api.test/final", "cors-final", Some("http://page.test")),
    );

    let mut browser = browser_for(
        r#"
            fetch('/start').then(function (response) {
                console.log(response.type);
                console.log(response.redirected);
                console.log(response.url);
                return response.text();
            }).then(function (text) { console.log(text); });
        "#,
        transport.clone(),
    );
    browser.settle_network(12);

    assert_eq!(
        browser.document().runtime.console,
        vec!["cors", "true", "http://api.test/final", "cors-final"]
    );
    let seen = transport.requests();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[1].headers.get("origin").as_deref(), Some("http://page.test"));
}

#[test]
fn final_cross_origin_response_still_requires_acao() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    transport.respond(
        "http://page.test/start",
        redirect("http://page.test/start", "http://api.test/final", None),
    );
    transport.respond(
        "http://api.test/final",
        ok("http://api.test/final", "secret", None),
    );

    let mut browser = browser_for(
        "fetch('/start').then(function () { console.log('unexpected'); }).catch(function () { console.log('blocked'); });",
        transport.clone(),
    );
    browser.settle_network(12);

    assert_eq!(browser.document().runtime.console, vec!["blocked"]);
    assert_eq!(transport.requests().len(), 2);
}

#[test]
fn cross_origin_redirect_response_must_pass_cors_before_following() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    transport.respond(
        "http://api.test/start",
        redirect("http://api.test/start", "http://cdn.test/final", None),
    );
    transport.respond(
        "http://cdn.test/final",
        ok("http://cdn.test/final", "must-not-load", Some("null")),
    );

    let mut browser = browser_for(
        "fetch('http://api.test/start').catch(function () { console.log('blocked'); });",
        transport.clone(),
    );
    browser.settle_network(12);

    assert_eq!(browser.document().runtime.console, vec!["blocked"]);
    assert_eq!(transport.requests().len(), 1);
}

#[test]
fn second_cross_origin_hop_uses_null_origin_redirect_taint() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    transport.respond(
        "http://api.test/start",
        redirect(
            "http://api.test/start",
            "http://cdn.test/final",
            Some("http://page.test"),
        ),
    );
    transport.respond(
        "http://cdn.test/final",
        ok("http://cdn.test/final", "tainted", Some("null")),
    );

    let mut browser = browser_for(
        r#"
            fetch('http://api.test/start').then(function (response) {
                console.log(response.type);
                return response.text();
            }).then(function (text) { console.log(text); });
        "#,
        transport.clone(),
    );
    browser.settle_network(12);

    assert_eq!(browser.document().runtime.console, vec!["cors", "tainted"]);
    let seen = transport.requests();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].headers.get("origin").as_deref(), Some("http://page.test"));
    assert_eq!(seen[1].headers.get("origin").as_deref(), Some("null"));
}

#[test]
fn redirect_that_would_need_preflight_fails_before_second_dispatch() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    transport.respond(
        "http://page.test/start",
        redirect("http://page.test/start", "http://api.test/final", None),
    );

    let mut browser = browser_for(
        r#"
            fetch('/start', { method: 'PUT', headers: { 'X-Token': 'secret' } })
                .then(function () { console.log('unexpected'); })
                .catch(function () { console.log('preflight-blocked'); });
        "#,
        transport.clone(),
    );
    browser.settle_network(12);

    assert_eq!(browser.document().runtime.console, vec!["preflight-blocked"]);
    assert_eq!(transport.requests().len(), 1);
}

#[test]
fn same_origin_mode_still_rejects_cross_origin_redirect() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    transport.respond(
        "http://page.test/start",
        redirect("http://page.test/start", "http://api.test/final", None),
    );

    let mut browser = browser_for(
        "fetch('/start', { mode: 'same-origin' }).catch(function () { console.log('blocked'); });",
        transport.clone(),
    );
    browser.settle_network(12);

    assert_eq!(browser.document().runtime.console, vec!["blocked"]);
    assert_eq!(transport.requests().len(), 1);
}
