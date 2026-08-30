use std::rc::Rc;

use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Url};
use browser_engine::Browser;

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn browser_for_page(page: &str, script: &str, transport: Rc<ManualNetwork>) -> Browser {
    let mut loader = MemoryLoader::new();
    loader.insert(page, format!("<script>{script}</script>"));
    Browser::open_with_single_hop_network(
        Box::new(loader),
        transport,
        &url(page),
        Rc::new(ManualClock::new()),
    )
    .expect("browser opens")
}

fn browser_for(script: &str, transport: Rc<ManualNetwork>) -> Browser {
    browser_for_page("http://page.test/index.html", script, transport)
}

fn redirect(endpoint: &str, location: &str) -> FetchResponse {
    let mut response = FetchResponse::synthetic(
        url(endpoint),
        302,
        Some("text/plain"),
        b"redirect".to_vec(),
    );
    response.headers.insert_raw("location", location);
    response
}

fn cors_permission(
    endpoint: &str,
    allow_origin: &str,
    max_age: &str,
    credentialed: bool,
) -> FetchResponse {
    let mut response = FetchResponse::synthetic(
        url(endpoint),
        200,
        Some("text/plain"),
        b"ok".to_vec(),
    );
    response
        .headers
        .insert_raw("access-control-allow-origin", allow_origin);
    response
        .headers
        .insert_raw("access-control-allow-methods", "PUT");
    response
        .headers
        .insert_raw("access-control-allow-headers", "x-token");
    response
        .headers
        .insert_raw("access-control-max-age", max_age);
    if credentialed {
        response
            .headers
            .insert_raw("access-control-allow-credentials", "true");
    }
    response
}

fn methods_and_urls(transport: &ManualNetwork) -> Vec<(String, String)> {
    transport
        .requests()
        .iter()
        .map(|request| {
            (
                request.method.as_str().to_string(),
                request.url.to_string(),
            )
        })
        .collect()
}

#[test]
fn redirect_target_preflights_then_sends_the_actual_request() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    transport.respond(
        "http://page.test/start",
        redirect("http://page.test/start", "http://api.test/data"),
    );
    transport.respond(
        "http://api.test/data",
        cors_permission("http://api.test/data", "http://page.test", "60", false),
    );

    let mut browser = browser_for(
        r#"
            fetch('/start', {
                method: 'PUT',
                headers: { 'X-Token': 'secret' },
                body: 'payload'
            }).then(function () { console.log('done'); })
              .catch(function () { console.log('blocked'); });
        "#,
        transport.clone(),
    );
    browser.settle_network(20);

    assert_eq!(browser.document().runtime.console, vec!["done"]);
    assert_eq!(
        methods_and_urls(&transport),
        vec![
            ("PUT".into(), "http://page.test/start".into()),
            ("OPTIONS".into(), "http://api.test/data".into()),
            ("PUT".into(), "http://api.test/data".into()),
        ]
    );
    let seen = transport.requests();
    assert_eq!(seen[1].headers.get("origin").as_deref(), Some("http://page.test"));
    assert_eq!(
        seen[1]
            .headers
            .get("access-control-request-method")
            .as_deref(),
        Some("PUT")
    );
    assert_eq!(
        seen[1]
            .headers
            .get("access-control-request-headers")
            .as_deref(),
        Some("x-token")
    );
}

#[test]
fn denied_redirect_preflight_never_dispatches_the_actual_target() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    transport.respond(
        "http://page.test/start",
        redirect("http://page.test/start", "http://api.test/data"),
    );
    let mut denied =
        cors_permission("http://api.test/data", "http://page.test", "60", false);
    denied.headers.delete("access-control-allow-headers");
    transport.respond("http://api.test/data", denied);

    let mut browser = browser_for(
        r#"
            fetch('/start', {
                method: 'PUT',
                headers: { 'X-Token': 'secret' },
                body: 'payload'
            }).then(function () { console.log('unexpected'); })
              .catch(function () { console.log('blocked'); });
        "#,
        transport.clone(),
    );
    browser.settle_network(20);

    assert_eq!(browser.document().runtime.console, vec!["blocked"]);
    assert_eq!(
        methods_and_urls(&transport),
        vec![
            ("PUT".into(), "http://page.test/start".into()),
            ("OPTIONS".into(), "http://api.test/data".into()),
        ]
    );
}

#[test]
fn direct_preflight_cache_entry_is_reused_after_a_redirect() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    transport.respond(
        "http://page.test/start",
        redirect("http://page.test/start", "http://api.test/data"),
    );
    transport.respond(
        "http://api.test/data",
        cors_permission("http://api.test/data", "http://page.test", "60", false),
    );

    let mut browser = browser_for(
        r#"
            fetch('http://api.test/data', {
                method: 'PUT',
                headers: { 'X-Token': 'first' },
                body: 'one'
            }).then(function () {
                return fetch('/start', {
                    method: 'PUT',
                    headers: { 'X-Token': 'second' },
                    body: 'two'
                });
            }).then(function () { console.log('done'); });
        "#,
        transport.clone(),
    );
    browser.settle_network(30);

    assert_eq!(browser.document().runtime.console, vec!["done"]);
    assert_eq!(
        methods_and_urls(&transport),
        vec![
            ("OPTIONS".into(), "http://api.test/data".into()),
            ("PUT".into(), "http://api.test/data".into()),
            ("PUT".into(), "http://page.test/start".into()),
            ("PUT".into(), "http://api.test/data".into()),
        ]
    );
}

#[test]
fn redirect_preflight_cache_entry_is_reused_by_a_later_direct_fetch() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    transport.respond(
        "http://page.test/start",
        redirect("http://page.test/start", "http://api.test/data"),
    );
    transport.respond(
        "http://api.test/data",
        cors_permission("http://api.test/data", "http://page.test", "60", false),
    );

    let mut browser = browser_for(
        r#"
            fetch('/start', {
                method: 'PUT',
                headers: { 'X-Token': 'first' },
                body: 'one'
            }).then(function () {
                return fetch('http://api.test/data', {
                    method: 'PUT',
                    headers: { 'X-Token': 'second' },
                    body: 'two'
                });
            }).then(function () { console.log('done'); });
        "#,
        transport.clone(),
    );
    browser.settle_network(30);

    assert_eq!(browser.document().runtime.console, vec!["done"]);
    assert_eq!(
        methods_and_urls(&transport),
        vec![
            ("PUT".into(), "http://page.test/start".into()),
            ("OPTIONS".into(), "http://api.test/data".into()),
            ("PUT".into(), "http://api.test/data".into()),
            ("PUT".into(), "http://api.test/data".into()),
        ]
    );
}

#[test]
fn credentialed_redirect_preflight_omits_cookie_then_actual_includes_it() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    transport.respond(
        "https://page.test/start",
        redirect("https://page.test/start", "https://api.test/data"),
    );
    transport.respond(
        "https://api.test/data",
        cors_permission("https://api.test/data", "https://page.test", "60", true),
    );

    let mut browser = browser_for_page(
        "https://page.test/index.html",
        r#"
            fetch('/start', {
                method: 'PUT',
                headers: { 'X-Token': 'secret' },
                body: 'payload',
                credentials: 'include'
            }).then(function () { console.log('done'); });
        "#,
        transport.clone(),
    );
    assert!(browser.cookie_jar().borrow_mut().store_set_cookie(
        "session=one; Path=/; Secure; SameSite=None",
        &url("https://api.test/"),
        0,
    ));

    browser.settle_network(20);

    assert_eq!(browser.document().runtime.console, vec!["done"]);
    let seen = transport.requests();
    assert_eq!(seen.len(), 3);
    assert_eq!(seen[1].method.as_str(), "OPTIONS");
    assert!(seen[1].headers.get("cookie").is_none());
    assert_eq!(seen[2].method.as_str(), "PUT");
    assert_eq!(seen[2].headers.get("cookie").as_deref(), Some("session=one"));
}
