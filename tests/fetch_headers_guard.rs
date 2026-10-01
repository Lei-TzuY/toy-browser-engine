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
fn network_response_headers_reject_every_script_mutator() {
    let endpoint = "http://page.test/data";
    let (mut browser, transport) = browser_for(
        r#"
            fetch('/data').then(function (response) {
                console.log('before:' + response.headers.get('x-test'));
                try {
                    response.headers.set('x-test', 'changed');
                    console.log('set-unexpected');
                } catch (error) {
                    console.log('set-blocked');
                }
                try {
                    response.headers.append('x-test', 'extra');
                    console.log('append-unexpected');
                } catch (error) {
                    console.log('append-blocked');
                }
                try {
                    response.headers.delete('x-test');
                    console.log('delete-unexpected');
                } catch (error) {
                    console.log('delete-blocked');
                }
                console.log('after:' + response.headers.get('x-test'));
            });
        "#,
    );

    let mut response =
        FetchResponse::synthetic(url(endpoint), 200, Some("text/plain"), b"ok".to_vec());
    response.headers.append_raw("x-test", "original");
    transport.respond(endpoint, response);

    settle(&mut browser, &transport);
    assert_eq!(
        browser.document().runtime.console,
        vec![
            "before:original",
            "set-blocked",
            "append-blocked",
            "delete-blocked",
            "after:original",
        ]
    );
}

#[test]
fn cloned_network_response_preserves_immutable_headers_guard() {
    let endpoint = "http://page.test/data";
    let (mut browser, transport) = browser_for(
        r#"
            fetch('/data').then(function (response) {
                var copy = response.clone();
                try {
                    copy.headers.set('x-test', 'changed');
                    console.log('clone-unexpected');
                } catch (error) {
                    console.log('clone-blocked');
                }
                console.log('original:' + response.headers.get('x-test'));
                console.log('copy:' + copy.headers.get('x-test'));
            });
        "#,
    );

    let mut response =
        FetchResponse::synthetic(url(endpoint), 200, Some("text/plain"), b"ok".to_vec());
    response.headers.append_raw("x-test", "original");
    transport.respond(endpoint, response);

    settle(&mut browser, &transport);
    assert_eq!(
        browser.document().runtime.console,
        vec!["clone-blocked", "original:original", "copy:original"]
    );
}

#[test]
fn synthetic_response_headers_remain_mutable() {
    let (browser, _) = browser_for(
        r#"
            var response = new Response('ok', { headers: { 'X-Test': 'one' } });
            response.headers.set('x-test', 'two');
            response.headers.append('x-test', 'three');
            console.log('joined:' + response.headers.get('x-test'));
            response.headers.delete('x-test');
            console.log('deleted:' + response.headers.has('x-test'));
        "#,
    );

    assert_eq!(
        browser.document().runtime.console,
        vec!["joined:two, three", "deleted:false"]
    );
}

#[test]
fn headers_constructor_copies_network_headers_into_a_mutable_list() {
    let endpoint = "http://page.test/data";
    let (mut browser, transport) = browser_for(
        r#"
            fetch('/data').then(function (response) {
                var copy = new Headers(response.headers);
                copy.set('x-test', 'changed');
                copy.append('x-copy', 'yes');
                console.log('copy:' + copy.get('x-test') + ':' + copy.get('x-copy'));
                console.log('network:' + response.headers.get('x-test'));
            });
        "#,
    );

    let mut response =
        FetchResponse::synthetic(url(endpoint), 200, Some("text/plain"), b"ok".to_vec());
    response.headers.append_raw("x-test", "original");
    transport.respond(endpoint, response);

    settle(&mut browser, &transport);
    assert_eq!(
        browser.document().runtime.console,
        vec!["copy:changed:yes", "network:original"]
    );
}

#[test]
fn opaque_response_headers_are_immutable_even_though_the_list_is_empty() {
    let endpoint = "http://api.test/opaque";
    let (mut browser, transport) = browser_for(
        r#"
            fetch('http://api.test/opaque', { mode: 'no-cors' }).then(function (response) {
                try {
                    response.headers.set('x-test', 'value');
                    console.log('opaque-unexpected');
                } catch (error) {
                    console.log('opaque-blocked');
                }
                console.log('visible:' + response.headers.has('x-test'));
            });
        "#,
    );
    transport.respond(
        endpoint,
        FetchResponse::synthetic(url(endpoint), 200, Some("text/plain"), b"secret".to_vec()),
    );

    settle(&mut browser, &transport);
    assert_eq!(
        browser.document().runtime.console,
        vec!["opaque-blocked", "visible:false"]
    );
}
