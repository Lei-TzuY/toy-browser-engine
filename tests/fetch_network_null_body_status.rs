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
fn status_204_response_has_no_body_stream_even_if_backend_supplies_bytes() {
    let endpoint = "http://page.test/no-content";
    let (mut browser, transport) = browser_for(
        r#"
            fetch('/no-content').then(function (response) {
                console.log('before:' + response.status + ':' + response.bodyUsed);
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
            204,
            Some("text/plain"),
            b"payload that a 204 must not expose".to_vec(),
        ),
    );

    settle(&mut browser, &transport);
    assert_eq!(
        browser.document().runtime.console,
        vec!["before:204:false", "after::false"]
    );
}

#[test]
fn status_304_response_and_clone_both_preserve_null_body_state() {
    let endpoint = "http://page.test/not-modified";
    let (mut browser, transport) = browser_for(
        r#"
            fetch('/not-modified').then(function (response) {
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
        FetchResponse::synthetic(
            url(endpoint),
            304,
            Some("text/plain"),
            b"not-modified payload must be ignored".to_vec(),
        ),
    );

    settle(&mut browser, &transport);
    assert_eq!(
        browser.document().runtime.console,
        vec!["original::false", "copy::false"]
    );
}

#[test]
fn ordinary_empty_200_response_still_has_a_consumable_body() {
    let endpoint = "http://page.test/empty";
    let (mut browser, transport) = browser_for(
        r#"
            fetch('/empty').then(function (response) {
                console.log('before:' + response.bodyUsed);
                response.text().then(function (text) {
                    console.log('after:' + text + ':' + response.bodyUsed);
                });
            });
        "#,
    );
    transport.respond(
        endpoint,
        FetchResponse::synthetic(url(endpoint), 200, Some("text/plain"), Vec::new()),
    );

    settle(&mut browser, &transport);
    assert_eq!(
        browser.document().runtime.console,
        vec!["before:false", "after::true"]
    );
}

#[test]
fn opaque_filtered_response_remains_bodyless_when_read_and_cloned() {
    let endpoint = "http://api.test/opaque";
    let (mut browser, transport) = browser_for(
        r#"
            fetch('http://api.test/opaque', { mode: 'no-cors' }).then(function (response) {
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
        FetchResponse::synthetic(
            url(endpoint),
            200,
            Some("text/plain"),
            b"cross-origin bytes".to_vec(),
        ),
    );

    settle(&mut browser, &transport);
    assert_eq!(
        browser.document().runtime.console,
        vec!["original::false", "copy::false"]
    );
}
