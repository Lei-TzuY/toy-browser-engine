use std::rc::Rc;

use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Url};
use browser_engine::Browser;

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn browser_for(script: &str, transport: Rc<ManualNetwork>) -> Browser {
    let page = "http://page.test/index.html";
    let mut loader = MemoryLoader::new();
    loader.insert(page, format!("<script>{script}</script>"));
    Browser::open_with_network(
        Box::new(loader),
        transport,
        &url(page),
        Rc::new(ManualClock::new()),
    )
    .expect("browser opens")
}

#[test]
fn fetch_request_disturbs_body_synchronously_before_network_turn() {
    let transport = Rc::new(ManualNetwork::new());
    let mut browser = browser_for(
        r#"
            var request = new Request('/submit', { method: 'POST', body: 'payload' });
            console.log('before:' + request.bodyUsed);
            fetch(request);
            console.log('after:' + request.bodyUsed);
        "#,
        transport.clone(),
    );

    assert_eq!(
        browser.document().runtime.console,
        vec!["before:false", "after:true"]
    );
    assert!(
        transport.requests().is_empty(),
        "Fetch still performs no transport I/O on the caller stack"
    );

    let turn = browser.tick();
    assert_eq!(turn.requests_sent, 1);
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method.as_str(), "POST");
    assert_eq!(requests[0].body.as_deref(), Some(b"payload".as_slice()));
}

#[test]
fn fetching_request_blocks_later_clone_and_body_read() {
    let transport = Rc::new(ManualNetwork::new());
    let mut browser = browser_for(
        r#"
            var request = new Request('/submit', { method: 'POST', body: 'payload' });
            fetch(request);
            try {
                request.clone();
                console.log('clone-unexpected');
            } catch (error) {
                console.log('clone-blocked');
            }
            request.text()
                .then(function () { console.log('read-unexpected'); })
                .catch(function () { console.log('read-blocked'); });
        "#,
        transport,
    );

    browser.tick();
    assert_eq!(
        browser.document().runtime.console,
        vec!["clone-blocked", "read-blocked"]
    );
}

#[test]
fn fetch_init_body_override_does_not_disturb_source_request() {
    let transport = Rc::new(ManualNetwork::new());
    let mut browser = browser_for(
        r#"
            var request = new Request('/submit', { method: 'POST', body: 'original' });
            fetch(request, { body: 'override' });
            console.log('source:' + request.bodyUsed);
        "#,
        transport.clone(),
    );

    assert_eq!(browser.document().runtime.console, vec!["source:false"]);
    browser.tick();
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body.as_deref(), Some(b"override".as_slice()));
}

#[test]
fn already_aborted_fetch_does_not_disturb_request_body() {
    let transport = Rc::new(ManualNetwork::new());
    let mut browser = browser_for(
        r#"
            var controller = new AbortController();
            controller.abort();
            var request = new Request('/submit', {
                method: 'POST',
                body: 'payload',
                signal: controller.signal
            });
            fetch(request).catch(function () { console.log('aborted'); });
            console.log('bodyUsed:' + request.bodyUsed);
        "#,
        transport.clone(),
    );

    browser.tick();
    assert_eq!(
        browser.document().runtime.console,
        vec!["bodyUsed:false", "aborted"]
    );
    assert!(transport.requests().is_empty());
}

#[test]
fn denied_preflight_does_not_restore_the_consumed_request_body() {
    let endpoint = "http://api.test/data";
    let transport = Rc::new(ManualNetwork::new());
    let mut denied = FetchResponse::synthetic(
        url(endpoint),
        200,
        Some("text/plain"),
        b"preflight".to_vec(),
    );
    denied
        .headers
        .append_raw("access-control-allow-origin", "*");
    denied
        .headers
        .append_raw("access-control-allow-methods", "GET");
    denied
        .headers
        .append_raw("access-control-allow-headers", "x-token");
    transport.respond(endpoint, denied);

    let mut browser = browser_for(
        r#"
            var request = new Request('http://api.test/data', {
                method: 'PUT',
                headers: { 'X-Token': 'secret' },
                body: 'payload'
            });
            fetch(request)
                .catch(function () { console.log('rejected:' + request.bodyUsed); });
            console.log('after-fetch:' + request.bodyUsed);
        "#,
        transport.clone(),
    );

    assert_eq!(
        browser.document().runtime.console,
        vec!["after-fetch:true"]
    );
    browser.tick();
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method.as_str(), "OPTIONS");
    assert!(requests[0].body.is_none());

    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    browser.tick();
    assert_eq!(transport.requests().len(), 1, "denied preflight sends no PUT");
    assert_eq!(
        browser.document().runtime.console,
        vec!["after-fetch:true", "rejected:true"]
    );
}

#[test]
fn cloned_body_branch_stays_usable_when_sibling_is_fetched() {
    let transport = Rc::new(ManualNetwork::new());
    let mut browser = browser_for(
        r#"
            var original = new Request('/submit', { method: 'POST', body: 'payload' });
            var copy = original.clone();
            fetch(original);
            console.log('original:' + original.bodyUsed);
            console.log('copy-before:' + copy.bodyUsed);
            fetch(copy);
            console.log('copy-after:' + copy.bodyUsed);
        "#,
        transport.clone(),
    );

    assert_eq!(
        browser.document().runtime.console,
        vec!["original:true", "copy-before:false", "copy-after:true"]
    );
    let turn = browser.tick();
    assert_eq!(turn.requests_sent, 2);
    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests
        .iter()
        .all(|request| request.body.as_deref() == Some(b"payload".as_slice())));
}

#[test]
fn explicit_empty_post_body_is_present_consumed_and_sent() {
    let transport = Rc::new(ManualNetwork::new());
    let mut browser = browser_for(
        r#"
            var request = new Request('/empty', { method: 'POST', body: '' });
            console.log('before:' + request.bodyUsed);
            fetch(request);
            console.log('after:' + request.bodyUsed);
        "#,
        transport.clone(),
    );

    assert_eq!(
        browser.document().runtime.console,
        vec!["before:false", "after:true"]
    );
    browser.tick();
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body.as_deref(), Some(&b""[..]));
}

#[test]
fn bodyless_post_request_has_no_body_stream_to_disturb() {
    let transport = Rc::new(ManualNetwork::new());
    let mut browser = browser_for(
        r#"
            var request = new Request('/bodyless', { method: 'POST' });
            fetch(request);
            console.log('bodyUsed:' + request.bodyUsed);
        "#,
        transport.clone(),
    );

    assert_eq!(browser.document().runtime.console, vec!["bodyUsed:false"]);
    browser.tick();
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].body.is_none());
}
