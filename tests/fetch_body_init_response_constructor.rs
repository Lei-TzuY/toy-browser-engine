use std::rc::Rc;

use browser_engine::eventloop::ManualClock;
use browser_engine::net::{ManualNetwork, MemoryLoader, Url};
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
fn synthetic_response_defaults_have_empty_url_status_text_and_null_body() {
    let transport = Rc::new(ManualNetwork::new());
    let mut browser = browser_for(
        r#"
            var response = new Response();
            console.log('url:' + response.url);
            console.log('status:' + response.status);
            console.log('statusText:' + response.statusText);
            console.log('bodyUsed-before:' + response.bodyUsed);
            response.text().then(function (text) {
                console.log('text:' + text);
                console.log('bodyUsed-after:' + response.bodyUsed);
            });
        "#,
        transport,
    );

    browser.tick();
    assert_eq!(
        browser.document().runtime.console,
        vec![
            "url:",
            "status:200",
            "statusText:",
            "bodyUsed-before:false",
            "text:",
            "bodyUsed-after:false",
        ]
    );
}

#[test]
fn string_body_gets_default_text_content_type_without_overriding_authored_header() {
    let transport = Rc::new(ManualNetwork::new());
    let browser = browser_for(
        r#"
            var response = new Response('hello');
            var explicit = new Response('hello', { headers: { 'Content-Type': 'application/custom' } });
            var request = new Request('/submit', { method: 'POST', body: 'payload' });
            var requestExplicit = new Request('/submit', {
                method: 'POST',
                body: 'payload',
                headers: { 'Content-Type': 'application/json' }
            });
            console.log('response:' + response.headers.get('content-type'));
            console.log('response-explicit:' + explicit.headers.get('content-type'));
            console.log('request:' + request.headers.get('content-type'));
            console.log('request-explicit:' + requestExplicit.headers.get('content-type'));
        "#,
        transport,
    );

    assert_eq!(
        browser.document().runtime.console,
        vec![
            "response:text/plain;charset=UTF-8",
            "response-explicit:application/custom",
            "request:text/plain;charset=UTF-8",
            "request-explicit:application/json",
        ]
    );
}

#[test]
fn url_search_params_body_serializes_and_sets_form_content_type() {
    let transport = Rc::new(ManualNetwork::new());
    let mut browser = browser_for(
        r#"
            var params = new URLSearchParams();
            params.append('name', 'Ada Lovelace');
            params.append('lang', 'C++');
            var request = new Request('/submit', { method: 'POST', body: params });
            var response = new Response(params);
            console.log('request-type:' + request.headers.get('content-type'));
            console.log('response-type:' + response.headers.get('content-type'));
            request.text().then(function (text) { console.log('request-body:' + text); });
            response.text().then(function (text) { console.log('response-body:' + text); });
        "#,
        transport,
    );

    browser.tick();
    assert_eq!(
        browser.document().runtime.console,
        vec![
            "request-type:application/x-www-form-urlencoded;charset=UTF-8",
            "response-type:application/x-www-form-urlencoded;charset=UTF-8",
            "request-body:name=Ada+Lovelace&lang=C%2B%2B",
            "response-body:name=Ada+Lovelace&lang=C%2B%2B",
        ]
    );
}

#[test]
fn string_request_content_type_reaches_transport() {
    let transport = Rc::new(ManualNetwork::new());
    let mut browser = browser_for(
        r#"
            fetch('/submit', { method: 'POST', body: 'payload' });
        "#,
        transport.clone(),
    );

    browser.tick();
    let requests = transport.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].headers.get("content-type").as_deref(),
        Some("text/plain;charset=UTF-8")
    );
    assert_eq!(requests[0].body.as_deref(), Some(b"payload".as_slice()));
}

#[test]
fn response_constructor_rejects_status_outside_200_through_599() {
    let transport = Rc::new(ManualNetwork::new());
    let browser = browser_for(
        r#"
            try { new Response(null, { status: 199 }); console.log('199-unexpected'); }
            catch (error) { console.log('199-blocked'); }
            try { new Response(null, { status: 600 }); console.log('600-unexpected'); }
            catch (error) { console.log('600-blocked'); }
        "#,
        transport,
    );

    assert_eq!(
        browser.document().runtime.console,
        vec!["199-blocked", "600-blocked"]
    );
}

#[test]
fn response_constructor_rejects_body_for_null_body_statuses() {
    let transport = Rc::new(ManualNetwork::new());
    let browser = browser_for(
        r#"
            try { new Response('x', { status: 204 }); console.log('204-unexpected'); }
            catch (error) { console.log('204-blocked'); }
            try { new Response('', { status: 205 }); console.log('205-unexpected'); }
            catch (error) { console.log('205-blocked'); }
            try { new Response('x', { status: 304 }); console.log('304-unexpected'); }
            catch (error) { console.log('304-blocked'); }
            var allowed = new Response(null, { status: 204 });
            console.log('allowed:' + allowed.status + ':' + allowed.bodyUsed);
        "#,
        transport,
    );

    assert_eq!(
        browser.document().runtime.console,
        vec!["204-blocked", "205-blocked", "304-blocked", "allowed:204:false"]
    );
}

#[test]
fn response_status_text_is_empty_by_default_and_valid_custom_text_is_preserved() {
    let transport = Rc::new(ManualNetwork::new());
    let browser = browser_for(
        r#"
            var normal = new Response(null, { status: 404 });
            var custom = new Response(null, { status: 418, statusText: 'Short and stout' });
            console.log('normal:' + normal.statusText);
            console.log('custom:' + custom.statusText);
            try { new Response(null, { statusText: 'bad\ntext' }); console.log('bad-unexpected'); }
            catch (error) { console.log('bad-blocked'); }
        "#,
        transport,
    );

    assert_eq!(
        browser.document().runtime.console,
        vec!["normal:", "custom:Short and stout", "bad-blocked"]
    );
}

#[test]
fn response_clone_preserves_a_null_body_instead_of_inventing_an_empty_stream() {
    let transport = Rc::new(ManualNetwork::new());
    let mut browser = browser_for(
        r#"
            var original = new Response();
            var copy = original.clone();
            copy.text().then(function (text) {
                console.log('copy:' + text + ':' + copy.bodyUsed);
                console.log('original:' + original.bodyUsed);
            });
        "#,
        transport,
    );

    browser.tick();
    assert_eq!(
        browser.document().runtime.console,
        vec!["copy::false", "original:false"]
    );
}
