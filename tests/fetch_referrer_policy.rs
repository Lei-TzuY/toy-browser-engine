use std::rc::Rc;

use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Url};
use browser_engine::Browser;

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn browser_for_html(page: &str, html: &str, transport: Rc<ManualNetwork>) -> Browser {
    let mut loader = MemoryLoader::new();
    loader.insert(page, html);
    Browser::open_with_single_hop_network(
        Box::new(loader),
        transport,
        &url(page),
        Rc::new(ManualClock::new()),
    )
    .expect("browser opens")
}

fn browser_for(page: &str, head: &str, script: &str, transport: Rc<ManualNetwork>) -> Browser {
    browser_for_html(
        page,
        &format!("<html><head>{head}</head><body><script>{script}</script></body></html>"),
        transport,
    )
}

fn ok(endpoint: &str) -> FetchResponse {
    FetchResponse::synthetic(url(endpoint), 200, Some("text/plain"), b"ok".to_vec())
}

fn cors_ok(endpoint: &str, origin: &str) -> FetchResponse {
    let mut response = ok(endpoint);
    response
        .headers
        .insert_raw("access-control-allow-origin", origin);
    response
}

fn permission(endpoint: &str, origin: &str) -> FetchResponse {
    let mut response = cors_ok(endpoint, origin);
    response
        .headers
        .insert_raw("access-control-allow-methods", "PUT");
    response
        .headers
        .insert_raw("access-control-allow-headers", "x-token");
    response
}

fn redirect(endpoint: &str, location: &str, policy: Option<&str>) -> FetchResponse {
    let mut response = FetchResponse::synthetic(
        url(endpoint),
        302,
        Some("text/plain"),
        b"redirect".to_vec(),
    );
    response.headers.insert_raw("location", location);
    if let Some(policy) = policy {
        response.headers.insert_raw("referrer-policy", policy);
    }
    response
}

#[test]
fn request_referrer_surface_is_inherited_and_overridable() {
    let transport = Rc::new(ManualNetwork::new());
    let browser = browser_for(
        "http://page.test/index.html",
        "",
        r#"
            let original = new Request('/one');
            let changed = new Request(original, {
                referrer: '',
                referrerPolicy: 'no-referrer'
            });
            let clone = new Request(changed);
            console.log(original.referrer === 'about:client');
            console.log(original.referrerPolicy === '');
            console.log(changed.referrer === '');
            console.log(changed.referrerPolicy === 'no-referrer');
            console.log(clone.referrer === '' && clone.referrerPolicy === 'no-referrer');
        "#,
        transport,
    );

    assert_eq!(
        browser.document().runtime.console,
        vec!["true", "true", "true", "true", "true"]
    );
}

#[test]
fn committed_document_not_base_url_is_client_referrer_and_security_origin() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    transport.respond(
        "http://base.test/assets/data",
        cors_ok("http://base.test/assets/data", "http://page.test"),
    );
    let mut browser = browser_for(
        "http://page.test/dir/index.html?q=1",
        r#"<base href="http://base.test/assets/"><meta name="referrer" content="unsafe-url">"#,
        "fetch('data').then(function () { console.log('done'); });",
        transport.clone(),
    );
    browser.settle_network(10);

    assert_eq!(browser.document().runtime.console, vec!["done"]);
    let seen = transport.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].url.to_string(), "http://base.test/assets/data");
    assert_eq!(
        seen[0].headers.get("origin").as_deref(),
        Some("http://page.test")
    );
    assert_eq!(
        seen[0].headers.get("referer").as_deref(),
        Some("http://page.test/dir/index.html?q=1")
    );
}

#[test]
fn empty_referrer_suppresses_document_unsafe_url_policy() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    transport.respond(
        "http://api.test/data",
        cors_ok("http://api.test/data", "http://page.test"),
    );
    let mut browser = browser_for(
        "http://page.test/private/index.html?q=1",
        r#"<meta name="referrer" content="unsafe-url">"#,
        "fetch('http://api.test/data', { referrer: '' }).then(function () { console.log('done'); });",
        transport.clone(),
    );
    browser.settle_network(10);

    assert_eq!(browser.document().runtime.console, vec!["done"]);
    assert!(transport.requests()[0].headers.get("referer").is_none());
}

#[test]
fn explicit_same_origin_referrer_is_fragmentless_and_cross_origin_referrer_is_rejected() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    transport.respond(
        "http://api.test/data",
        cors_ok("http://api.test/data", "http://page.test"),
    );
    let mut browser = browser_for(
        "http://page.test/index.html",
        "",
        r#"
            fetch('http://api.test/data', {
                referrer: '/private/source?q=1#secret',
                referrerPolicy: 'unsafe-url'
            }).then(function () { console.log('first'); });
            fetch('http://api.test/never', {
                referrer: 'http://other.test/private'
            }).catch(function () { console.log('blocked'); });
        "#,
        transport.clone(),
    );
    browser.settle_network(10);

    assert!(browser.document().runtime.console.contains(&"first".to_string()));
    assert!(browser.document().runtime.console.contains(&"blocked".to_string()));
    let seen = transport.requests();
    assert_eq!(seen.len(), 1, "bad explicit referrer must fail before transport");
    assert_eq!(
        seen[0].headers.get("referer").as_deref(),
        Some("http://page.test/private/source?q=1")
    );
}

#[test]
fn redirect_response_recomputes_referer_from_stable_source() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    transport.respond(
        "http://page.test/start",
        redirect("http://page.test/start", "http://api.test/data", Some("origin")),
    );
    transport.respond(
        "http://api.test/data",
        cors_ok("http://api.test/data", "http://page.test"),
    );
    let mut browser = browser_for(
        "http://page.test/private/index.html?q=1",
        r#"<meta name="referrer" content="unsafe-url">"#,
        "fetch('/start').then(function () { console.log('done'); });",
        transport.clone(),
    );
    browser.settle_network(20);

    assert_eq!(browser.document().runtime.console, vec!["done"]);
    let seen = transport.requests();
    assert_eq!(seen.len(), 2);
    assert_eq!(
        seen[0].headers.get("referer").as_deref(),
        Some("http://page.test/private/index.html?q=1")
    );
    assert_eq!(
        seen[1].headers.get("referer").as_deref(),
        Some("http://page.test/")
    );
}

#[test]
fn initial_preflight_inherits_referrer_but_preflight_response_policy_does_not_mutate_it() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    let mut allowed = permission("http://api.test/data", "http://page.test");
    allowed
        .headers
        .insert_raw("referrer-policy", "no-referrer");
    transport.respond("http://api.test/data", allowed);

    let mut browser = browser_for(
        "http://page.test/private/index.html?q=1",
        r#"<meta name="referrer" content="unsafe-url">"#,
        r#"
            fetch('http://api.test/data', {
                method: 'PUT',
                headers: { 'X-Token': 'secret' },
                body: 'payload'
            }).then(function () { console.log('done'); });
        "#,
        transport.clone(),
    );
    browser.settle_network(20);

    assert_eq!(browser.document().runtime.console, vec!["done"]);
    let seen = transport.requests();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].method.as_str(), "OPTIONS");
    assert_eq!(seen[1].method.as_str(), "PUT");
    for request in &seen {
        assert_eq!(
            request.headers.get("referer").as_deref(),
            Some("http://page.test/private/index.html?q=1")
        );
    }
}

#[test]
fn redirect_target_preflight_uses_policy_updated_by_redirect_response() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    transport.respond(
        "http://page.test/start",
        redirect("http://page.test/start", "http://api.test/data", Some("origin")),
    );
    transport.respond(
        "http://api.test/data",
        permission("http://api.test/data", "http://page.test"),
    );

    let mut browser = browser_for(
        "http://page.test/private/index.html?q=1",
        r#"<meta name="referrer" content="unsafe-url">"#,
        r#"
            fetch('/start', {
                method: 'PUT',
                headers: { 'X-Token': 'secret' },
                body: 'payload'
            }).then(function () { console.log('done'); });
        "#,
        transport.clone(),
    );
    browser.settle_network(20);

    assert_eq!(browser.document().runtime.console, vec!["done"]);
    let seen = transport.requests();
    assert_eq!(seen.len(), 3);
    assert_eq!(seen[0].method.as_str(), "PUT");
    assert_eq!(seen[1].method.as_str(), "OPTIONS");
    assert_eq!(seen[2].method.as_str(), "PUT");
    assert_eq!(
        seen[0].headers.get("referer").as_deref(),
        Some("http://page.test/private/index.html?q=1")
    );
    for request in &seen[1..] {
        assert_eq!(
            request.headers.get("referer").as_deref(),
            Some("http://page.test/")
        );
    }
}
