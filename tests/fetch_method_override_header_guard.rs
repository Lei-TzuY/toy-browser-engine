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
fn request_constructor_filters_forbidden_method_override_values() {
    let (browser, _) = browser_for(
        r#"
            var request = new Request('/data', { headers: {
                'X-HTTP-Method': 'TRACE',
                'X-HTTP-Method-Override': 'GET,track ',
                'X-Method-Override': ' connect',
                'Set-Cookie': 'session=bad'
            }});
            console.log(request.headers.get('x-http-method') === null ? 'method-blocked' : 'method-leak');
            console.log(request.headers.get('x-http-method-override') === null ? 'override-blocked' : 'override-leak');
            console.log(request.headers.get('x-method-override') === null ? 'short-blocked' : 'short-leak');
            console.log(request.headers.get('set-cookie') === null ? 'set-cookie-blocked' : 'set-cookie-leak');
        "#,
    );

    assert_eq!(
        browser.document().runtime.console,
        vec![
            "method-blocked",
            "override-blocked",
            "short-blocked",
            "set-cookie-blocked",
        ]
    );
}

#[test]
fn request_constructor_keeps_permitted_override_values() {
    let (browser, _) = browser_for(
        r#"
            var request = new Request('/data', { headers: {
                'X-HTTP-Method': 'GETTRACE',
                'X-HTTP-Method-Override': 'GET',
                'X-Method-Override': '"TRACE",'
            }});
            console.log('method:' + request.headers.get('x-http-method'));
            console.log('override:' + request.headers.get('x-http-method-override'));
            console.log('short:' + request.headers.get('x-method-override'));
        "#,
    );

    assert_eq!(
        browser.document().runtime.console,
        vec!["method:GETTRACE", "override:GET", "short:\"TRACE\","]
    );
}

#[test]
fn request_mutators_reject_forbidden_override_tokens_without_losing_old_value() {
    let (browser, _) = browser_for(
        r#"
            var request = new Request('/data', { headers: { 'X-HTTP-Method': 'GET' } });
            request.headers.set('x-http-method', 'TRACE');
            console.log('after-set:' + request.headers.get('x-http-method'));
            request.headers.append('x-http-method', 'post');
            console.log('after-safe-append:' + request.headers.get('x-http-method'));
            request.headers.append('x-http-method', 'TrAcK');
            console.log('after-blocked-append:' + request.headers.get('x-http-method'));
        "#,
    );

    assert_eq!(
        browser.document().runtime.console,
        vec![
            "after-set:GET",
            "after-safe-append:GET, post",
            "after-blocked-append:GET, post",
        ]
    );
}

#[test]
fn method_override_matching_is_case_insensitive_and_ows_tolerant() {
    let (browser, _) = browser_for(
        r#"
            var request = new Request('/data');
            request.headers.set('X-Method-Override', '  cOnNeCt  ');
            console.log(request.headers.has('x-method-override') ? 'connect-leak' : 'connect-blocked');
            request.headers.set('X-Method-Override', 'POST,\tTRACE\t');
            console.log(request.headers.has('x-method-override') ? 'trace-leak' : 'trace-blocked');
        "#,
    );

    assert_eq!(
        browser.document().runtime.console,
        vec!["connect-blocked", "trace-blocked"]
    );
}

#[test]
fn request_guard_allows_deleting_a_permitted_override_header() {
    let (browser, _) = browser_for(
        r#"
            var request = new Request('/data', { headers: { 'X-HTTP-Method-Override': 'PATCH' } });
            request.headers.delete('x-http-method-override');
            console.log(request.headers.has('x-http-method-override') ? 'delete-blocked' : 'delete-ok');
        "#,
    );

    assert_eq!(browser.document().runtime.console, vec!["delete-ok"]);
}

#[test]
fn standalone_headers_do_not_inherit_the_request_guard() {
    let (browser, _) = browser_for(
        r#"
            var headers = new Headers();
            headers.set('X-HTTP-Method-Override', 'TRACE');
            headers.set('Set-Cookie', 'standalone=yes');
            console.log('override:' + headers.get('x-http-method-override'));
            console.log('cookie:' + headers.get('set-cookie'));
        "#,
    );

    assert_eq!(
        browser.document().runtime.console,
        vec!["override:TRACE", "cookie:standalone=yes"]
    );
}

#[test]
fn copying_request_headers_to_headers_restores_mutability() {
    let (browser, _) = browser_for(
        r#"
            var request = new Request('/data', { headers: { 'X-HTTP-Method': 'GET' } });
            var copy = new Headers(request.headers);
            copy.set('x-http-method', 'TRACE');
            console.log('copy:' + copy.get('x-http-method'));
            console.log('request:' + request.headers.get('x-http-method'));
        "#,
    );

    assert_eq!(
        browser.document().runtime.console,
        vec!["copy:TRACE", "request:GET"]
    );
}

#[test]
fn wire_request_sends_permitted_override_value_but_not_forbidden_one() {
    let endpoint = "http://page.test/data";
    let (mut browser, transport) = browser_for(
        r#"
            var request = new Request('/data', { headers: {
                'X-HTTP-Method': 'PATCH',
                'X-HTTP-Method-Override': 'TRACE'
            }});
            request.headers.set('x-method-override', 'CONNECT');
            fetch(request).then(function (response) { console.log(response.status); });
        "#,
    );
    transport.respond(
        endpoint,
        FetchResponse::synthetic(url(endpoint), 204, None, Vec::new()),
    );

    assert_eq!(browser.tick().requests_sent, 1);
    let sent = transport.requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].headers.get("x-http-method").as_deref(),
        Some("PATCH")
    );
    assert!(sent[0].headers.get("x-http-method-override").is_none());
    assert!(sent[0].headers.get("x-method-override").is_none());

    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(browser.document().runtime.console, vec!["204"]);
}

#[test]
fn quoted_commas_do_not_create_false_forbidden_method_tokens() {
    let (browser, _) = browser_for(
        r#"
            var request = new Request('/data');
            request.headers.set('x-http-method-override', '"GET, TRACE, POST"');
            console.log('quoted:' + request.headers.get('x-http-method-override'));
            request.headers.append('x-http-method-override', 'CONNECT');
            console.log('after:' + request.headers.get('x-http-method-override'));
        "#,
    );

    assert_eq!(
        browser.document().runtime.console,
        vec!["quoted:\"GET, TRACE, POST\"", "after:\"GET, TRACE, POST\"",]
    );
}

#[test]
fn request_guards_use_the_normalized_header_name() {
    let (browser, _) = browser_for(
        r#"
            var request = new Request('/data');
            request.headers.set(' Cookie ', 'session=bad');
            console.log(request.headers.has('cookie') ? 'cookie-leak' : 'cookie-blocked');

            var noCors = new Request('/data', { mode: 'no-cors' });
            noCors.headers.set(' Accept ', 'text/html');
            console.log('accept:' + noCors.headers.get('accept'));
            noCors.headers.delete(' Accept ');
            console.log(noCors.headers.has('accept') ? 'delete-failed' : 'delete-ok');
        "#,
    );

    assert_eq!(
        browser.document().runtime.console,
        vec!["cookie-blocked", "accept:text/html", "delete-ok"]
    );
}
