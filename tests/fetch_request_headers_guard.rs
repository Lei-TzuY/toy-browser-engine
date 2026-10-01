use std::rc::Rc;

use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Url};
use browser_engine::Browser;

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn browser_for(script: &str) -> (Browser, Rc<ManualNetwork>) {
    let page = "http://page.test/index.html";
    let mut loader = MemoryLoader::new();
    loader.insert(page, format!("<script>{script}</script>"));
    let transport = Rc::new(ManualNetwork::new());
    let browser = Browser::open_with_network(
        Box::new(loader),
        transport.clone(),
        &url(page),
        Rc::new(ManualClock::new()),
    )
    .expect("browser opens");
    (browser, transport)
}

#[test]
fn request_guard_blocks_forbidden_names_but_keeps_custom_headers_mutable() {
    let (browser, _) = browser_for(
        r#"
            var request = new Request('/data', {
                headers: {
                    'X-Test': 'one',
                    'Origin': 'http://evil.test',
                    'Cookie': 'session=secret',
                    'Sec-Fetch-Site': 'cross-site'
                }
            });
            console.log(request.headers.get('origin') === null ? 'origin-filtered' : 'origin-leak');
            console.log(request.headers.get('cookie') === null ? 'cookie-filtered' : 'cookie-leak');
            console.log(request.headers.get('sec-fetch-site') === null ? 'sec-filtered' : 'sec-leak');
            request.headers.set('x-test', 'two');
            request.headers.set('origin', 'http://other.test');
            request.headers.append('cookie', 'other=value');
            console.log('custom:' + request.headers.get('x-test'));
            console.log(request.headers.get('origin') === null ? 'origin-still-filtered' : 'origin-mutated');
        "#,
    );

    assert_eq!(
        browser.document().runtime.console,
        vec![
            "origin-filtered",
            "cookie-filtered",
            "sec-filtered",
            "custom:two",
            "origin-still-filtered",
        ]
    );
}

#[test]
fn no_cors_constructor_immediately_exposes_only_safelisted_headers() {
    let (browser, _) = browser_for(
        r#"
            var request = new Request('http://api.test/data', {
                mode: 'no-cors',
                method: 'POST',
                headers: {
                    'X-Token': 'secret',
                    'Content-Type': 'application/json',
                    'Accept': 'text/html',
                    'Range': 'bytes=0-99'
                },
                body: 'payload'
            });
            console.log(request.headers.get('x-token') === null ? 'unsafe-filtered' : 'unsafe-leak');
            console.log(request.headers.get('content-type') === null ? 'type-filtered' : 'type-leak');
            console.log('accept:' + request.headers.get('accept'));
            console.log(request.headers.get('range') === null ? 'range-filtered' : 'range-leak');
        "#,
    );

    assert_eq!(
        browser.document().runtime.console,
        vec![
            "unsafe-filtered",
            "type-filtered",
            "accept:text/html",
            "range-filtered",
        ]
    );
}

#[test]
fn no_cors_mutators_ignore_unsafe_changes_and_allow_safe_changes() {
    let (browser, _) = browser_for(
        r#"
            var request = new Request('/data', {
                mode: 'no-cors',
                headers: { 'Accept': 'text/plain', 'Content-Type': 'text/plain' }
            });
            request.headers.set('x-token', 'secret');
            request.headers.set('content-type', 'application/json');
            request.headers.append('accept', 'text/html');
            console.log(request.headers.get('x-token') === null ? 'x-blocked' : 'x-leak');
            console.log('type:' + request.headers.get('content-type'));
            console.log('accept:' + request.headers.get('accept'));
            request.headers.delete('accept');
            console.log(request.headers.has('accept') ? 'delete-failed' : 'delete-ok');
        "#,
    );

    assert_eq!(
        browser.document().runtime.console,
        vec![
            "x-blocked",
            "type:text/plain",
            "accept:text/plain, text/html",
            "delete-ok",
        ]
    );
}

#[test]
fn no_cors_append_checks_the_combined_value_limit() {
    let long_accept = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let script = format!(
        r#"
            var request = new Request('/data', {{
                mode: 'no-cors',
                headers: {{ 'Accept': '{long_accept}' }}
            }});
            request.headers.append('accept', 'bbbbbbbbbb');
            console.log(request.headers.get('accept'));
        "#
    );
    let (browser, _) = browser_for(&script);

    assert_eq!(browser.document().runtime.console, vec![long_accept]);
}

#[test]
fn no_cors_script_cannot_create_privileged_range_header() {
    let (browser, _) = browser_for(
        r#"
            var request = new Request('/data', { mode: 'no-cors' });
            request.headers.set('range', 'bytes=0-99');
            console.log(request.headers.get('range') === null ? 'set-blocked' : 'set-leak');
            request.headers.append('range', 'bytes=200-299');
            console.log(request.headers.get('range') === null ? 'append-blocked' : 'append-leak');
        "#,
    );

    assert_eq!(
        browser.document().runtime.console,
        vec!["set-blocked", "append-blocked"]
    );
}

#[test]
fn cors_range_remains_safelisted_for_cross_origin_preflight_classification() {
    let endpoint = "http://api.test/data";
    let (mut browser, transport) = browser_for(
        r#"
            fetch('http://api.test/data', {
                headers: { 'Range': 'bytes=0-99' }
            }).then(function (response) { console.log(response.status); });
        "#,
    );
    let mut response =
        FetchResponse::synthetic(url(endpoint), 200, Some("text/plain"), b"ok".to_vec());
    response
        .headers
        .append_raw("access-control-allow-origin", "*");
    transport.respond(endpoint, response);

    assert_eq!(browser.tick().requests_sent, 1);
    let sent = transport.requests();
    assert_eq!(sent.len(), 1, "a safelisted Range must not trigger OPTIONS");
    assert_eq!(sent[0].headers.get("range").as_deref(), Some("bytes=0-99"));

    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(browser.document().runtime.console, vec!["200"]);
}

#[test]
fn request_clone_preserves_guard_and_detaches_header_storage() {
    let (browser, _) = browser_for(
        r#"
            var original = new Request('/data', {
                mode: 'no-cors',
                headers: { 'Accept': 'text/plain' }
            });
            var copy = original.clone();
            copy.headers.set('x-secret', 'blocked');
            copy.headers.set('accept', 'application/json');
            console.log('original:' + original.headers.get('accept'));
            console.log('copy:' + copy.headers.get('accept'));
            console.log(copy.headers.get('x-secret') === null ? 'guard-kept' : 'guard-lost');
        "#,
    );

    assert_eq!(
        browser.document().runtime.console,
        vec!["original:text/plain", "copy:application/json", "guard-kept"]
    );
}

#[test]
fn overriding_an_existing_request_to_no_cors_reapplies_the_stricter_guard() {
    let (browser, _) = browser_for(
        r#"
            var cors = new Request('/data', { headers: { 'X-Token': 'secret' } });
            var noCors = new Request(cors, { mode: 'no-cors' });
            console.log('cors:' + cors.headers.get('x-token'));
            console.log(noCors.headers.get('x-token') === null ? 'copy-filtered' : 'copy-leak');
            noCors.headers.set('x-token', 'again');
            console.log(noCors.headers.get('x-token') === null ? 'mutation-blocked' : 'mutation-leak');
        "#,
    );

    assert_eq!(
        browser.document().runtime.console,
        vec!["cors:secret", "copy-filtered", "mutation-blocked"]
    );
}

#[test]
fn no_cors_wire_request_keeps_safe_mutations_without_preflight() {
    let endpoint = "http://api.test/data";
    let (mut browser, transport) = browser_for(
        r#"
            var request = new Request('http://api.test/data', { mode: 'no-cors' });
            request.headers.set('accept', 'application/json');
            request.headers.set('x-token', 'secret');
            fetch(request).then(function (response) { console.log(response.type); });
        "#,
    );
    transport.respond(
        endpoint,
        FetchResponse::synthetic(url(endpoint), 200, Some("text/plain"), b"ok".to_vec()),
    );

    assert_eq!(browser.tick().requests_sent, 1);
    let sent = transport.requests();
    assert_eq!(
        sent.len(),
        1,
        "request-no-cors must not create OPTIONS preflight"
    );
    assert_eq!(
        sent[0].headers.get("accept").as_deref(),
        Some("application/json")
    );
    assert!(sent[0].headers.get("x-token").is_none());

    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(browser.document().runtime.console, vec!["opaque"]);
}
