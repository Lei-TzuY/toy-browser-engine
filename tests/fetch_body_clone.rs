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
    Browser::open_with_single_hop_network(
        Box::new(loader),
        transport,
        &url(page),
        Rc::new(ManualClock::new()),
    )
    .expect("browser opens")
}

fn response(endpoint: &str, body: &[u8]) -> FetchResponse {
    FetchResponse::synthetic(url(endpoint), 200, Some("text/plain"), body.to_vec())
}

#[test]
fn request_clone_has_independent_body_and_headers() {
    let transport = Rc::new(ManualNetwork::new());
    let browser = browser_for(
        r#"
            var original = new Request('/submit', {
                method: 'POST',
                body: 'alpha',
                headers: { 'X-Test': 'one' }
            });
            var copy = original.clone();
            copy.headers.set('X-Test', 'two');

            console.log(original.headers.get('X-Test'));
            console.log(copy.headers.get('X-Test'));
            console.log(original.bodyUsed);
            console.log(copy.bodyUsed);

            original.text().then(function (text) { console.log('original:' + text); });
            copy.text().then(function (text) { console.log('copy:' + text); });
            console.log(original.bodyUsed);
            console.log(copy.bodyUsed);
        "#,
        transport,
    );

    assert_eq!(
        browser.document().runtime.console,
        vec![
            "one",
            "two",
            "false",
            "false",
            "true",
            "true",
            "original:alpha",
            "copy:alpha",
        ]
    );
}

#[test]
fn response_clone_has_independent_body_and_headers() {
    let transport = Rc::new(ManualNetwork::new());
    let browser = browser_for(
        r#"
            var original = new Response('payload', { headers: { 'X-Test': 'one' } });
            var copy = original.clone();
            copy.headers.set('X-Test', 'two');

            console.log(original.headers.get('X-Test'));
            console.log(copy.headers.get('X-Test'));
            original.text().then(function (text) { console.log('original:' + text); });
            copy.text().then(function (text) { console.log('copy:' + text); });
            console.log(original.bodyUsed);
            console.log(copy.bodyUsed);
        "#,
        transport,
    );

    assert_eq!(
        browser.document().runtime.console,
        vec![
            "one",
            "two",
            "true",
            "true",
            "original:payload",
            "copy:payload",
        ]
    );
}

#[test]
fn request_clone_after_body_consumption_throws() {
    let transport = Rc::new(ManualNetwork::new());
    let browser = browser_for(
        r#"
            var request = new Request('/submit', { method: 'POST', body: 'used' });
            request.text();
            try {
                request.clone();
                console.log('unexpected');
            } catch (error) {
                console.log('blocked');
            }
        "#,
        transport,
    );

    assert_eq!(browser.document().runtime.console, vec!["blocked"]);
}

#[test]
fn response_clone_after_body_consumption_throws() {
    let transport = Rc::new(ManualNetwork::new());
    let browser = browser_for(
        r#"
            var response = new Response('used');
            response.text();
            try {
                response.clone();
                console.log('unexpected');
            } catch (error) {
                console.log('blocked');
            }
        "#,
        transport,
    );

    assert_eq!(browser.document().runtime.console, vec!["blocked"]);
}

#[test]
fn used_request_cannot_be_reconstructed_or_fetched() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    let mut browser = browser_for(
        r#"
            var request = new Request('/submit', { method: 'POST', body: 'used' });
            request.text();

            try {
                new Request(request);
                console.log('constructor-unexpected');
            } catch (error) {
                console.log('constructor-blocked');
            }

            fetch(request)
                .then(function () { console.log('fetch-unexpected'); })
                .catch(function () { console.log('fetch-blocked'); });
        "#,
        transport.clone(),
    );
    browser.settle_network(5);

    assert_eq!(
        browser.document().runtime.console,
        vec!["constructor-blocked", "fetch-blocked"]
    );
    assert!(transport.requests().is_empty());
}

#[test]
fn request_clone_preserves_policy_metadata_and_abort_signal() {
    let transport = Rc::new(ManualNetwork::new());
    let browser = browser_for(
        r#"
            var controller = new AbortController();
            var original = new Request('/data', {
                method: 'POST',
                body: 'payload',
                mode: 'cors',
                credentials: 'omit',
                redirect: 'follow',
                referrerPolicy: 'no-referrer',
                integrity: 'sha256-example',
                signal: controller.signal
            });
            var copy = original.clone();

            console.log(copy.method);
            console.log(copy.mode);
            console.log(copy.credentials);
            console.log(copy.redirect);
            console.log(copy.referrerPolicy);
            console.log(copy.integrity);
            console.log(copy.signal.aborted);
            controller.abort();
            console.log(copy.signal.aborted);
        "#,
        transport,
    );

    assert_eq!(
        browser.document().runtime.console,
        vec![
            "POST",
            "cors",
            "omit",
            "follow",
            "no-referrer",
            "sha256-example",
            "false",
            "true",
        ]
    );
}

#[test]
fn cloned_request_can_fetch_after_original_body_is_consumed() {
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    transport.respond(
        "http://page.test/submit",
        response("http://page.test/submit", b"ok"),
    );

    let mut browser = browser_for(
        r#"
            var original = new Request('/submit', { method: 'POST', body: 'alpha' });
            var copy = original.clone();
            original.text();
            fetch(copy)
                .then(function (response) { return response.text(); })
                .then(function (text) { console.log(text); })
                .catch(function () { console.log('blocked'); });
        "#,
        transport.clone(),
    );
    browser.settle_network(10);

    assert_eq!(browser.document().runtime.console, vec!["ok"]);
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method.as_str(), "POST");
    assert_eq!(requests[0].body.as_deref(), Some(b"alpha".as_slice()));
}
