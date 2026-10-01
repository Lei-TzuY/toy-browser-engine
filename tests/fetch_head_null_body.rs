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

fn settle(browser: &mut Browser, transport: &ManualNetwork) {
    assert_eq!(browser.tick().requests_sent, 1);
    assert_eq!(transport.complete_all(), 1);
    browser.tick();
}

#[test]
fn head_200_response_is_bodyless_even_if_backend_supplies_bytes() {
    let endpoint = "http://page.test/meta";
    let (mut browser, transport) = browser_for(
        r#"
            fetch('/meta', { method: 'HEAD' }).then(function (response) {
                console.log('before:' + response.status + ':' + response.bodyUsed);
                console.log('type:' + response.headers.get('content-type'));
                response.text().then(function (text) {
                    console.log('after:' + text + ':' + response.bodyUsed);
                });
            });
        "#,
    );
    transport.respond(
        endpoint,
        FetchResponse::synthetic(
            url(endpoint),
            200,
            Some("text/plain"),
            b"backend bytes must not become a HEAD body".to_vec(),
        ),
    );

    settle(&mut browser, &transport);
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method.as_str(), "HEAD");
    assert_eq!(
        browser.document().runtime.console,
        vec!["before:200:false", "type:text/plain", "after::false"]
    );
}

#[test]
fn head_response_clone_preserves_null_body_state() {
    let endpoint = "http://page.test/meta";
    let (mut browser, transport) = browser_for(
        r#"
            fetch('/meta', { method: 'HEAD' }).then(function (response) {
                var copy = response.clone();
                response.text().then(function (originalText) {
                    copy.text().then(function (copyText) {
                        console.log('original:' + originalText + ':' + response.bodyUsed);
                        console.log('copy:' + copyText + ':' + copy.bodyUsed);
                    });
                });
            });
        "#,
    );
    transport.respond(
        endpoint,
        FetchResponse::synthetic(url(endpoint), 200, None, b"ignored".to_vec()),
    );

    settle(&mut browser, &transport);
    assert_eq!(
        browser.document().runtime.console,
        vec!["original::false", "copy::false"]
    );
}

#[test]
fn cross_origin_cors_head_response_is_also_bodyless() {
    let endpoint = "http://api.test/meta";
    let (mut browser, transport) = browser_for(
        r#"
            fetch('http://api.test/meta', { method: 'HEAD' }).then(function (response) {
                console.log('type:' + response.type + ':' + response.bodyUsed);
                response.text().then(function (text) {
                    console.log('body:' + text + ':' + response.bodyUsed);
                });
            });
        "#,
    );
    let mut response = FetchResponse::synthetic(
        url(endpoint),
        200,
        Some("text/plain"),
        b"cors HEAD bytes must stay hidden".to_vec(),
    );
    response
        .headers
        .append_raw("access-control-allow-origin", "*");
    transport.respond(endpoint, response);

    settle(&mut browser, &transport);
    let requests = transport.requests();
    assert_eq!(
        requests.len(),
        1,
        "HEAD is CORS-safelisted and needs no preflight"
    );
    assert_eq!(requests[0].method.as_str(), "HEAD");
    assert_eq!(
        requests[0].headers.get("origin").as_deref(),
        Some("http://page.test")
    );
    assert_eq!(
        browser.document().runtime.console,
        vec!["type:cors:false", "body::false"]
    );
}
