use std::rc::Rc;

use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Url};
use browser_engine::script::ResponseData;
use browser_engine::Browser;

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

#[test]
fn basic_network_response_filters_cookie_headers_before_script_wrapping() {
    let mut wire = FetchResponse::synthetic(
        url("http://page.test/data"),
        200,
        Some("text/plain"),
        b"visible body".to_vec(),
    );
    wire.headers
        .append_raw("set-cookie", "session=secret; Path=/");
    wire.headers
        .append_raw("Set-Cookie", "theme=dark; Path=/");
    wire.headers.append_raw("set-cookie2", "legacy=secret");
    wire.headers.append_raw("x-visible", "yes");

    let response = ResponseData::from_wire(wire);
    let headers = response.headers.borrow();

    assert_eq!(headers.get("set-cookie"), None);
    assert_eq!(headers.get("set-cookie2"), None);
    assert_eq!(headers.get("x-visible").as_deref(), Some("yes"));
    assert_eq!(headers.get("content-type").as_deref(), Some("text/plain"));
}

#[test]
fn forbidden_header_filter_composes_with_null_body_statuses() {
    let mut wire = FetchResponse::synthetic(
        url("http://page.test/no-content"),
        204,
        Some("text/plain"),
        b"backend bytes must stay hidden".to_vec(),
    );
    wire.headers.append_raw("set-cookie", "session=secret");
    wire.headers.append_raw("x-request-id", "abc123");

    let response = ResponseData::from_wire(wire);

    assert_eq!(response.headers.borrow().get("set-cookie"), None);
    assert_eq!(
        response.headers.borrow().get("x-request-id").as_deref(),
        Some("abc123")
    );
    assert!(!response.body.present());
    assert_eq!(response.body.take().unwrap(), Vec::<u8>::new());
    assert!(!response.body.used());
}

#[test]
fn same_origin_fetch_never_exposes_cookie_response_headers_to_script() {
    let page = "http://page.test/index.html";
    let endpoint = "http://page.test/data";
    let mut loader = MemoryLoader::new();
    loader.insert(
        page,
        r#"
            <script>
            fetch('/data').then(function (response) {
                console.log('cookie:' + response.headers.has('set-cookie'));
                console.log('cookie2:' + response.headers.has('set-cookie2'));
                console.log('visible:' + response.headers.get('x-visible'));
                response.text().then(function (text) {
                    console.log('body:' + text);
                });
            });
            </script>
        "#,
    );

    let transport = Rc::new(ManualNetwork::new());
    let mut browser = Browser::open_with_network(
        Box::new(loader),
        transport.clone(),
        &url(page),
        Rc::new(ManualClock::new()),
    )
    .expect("browser opens");

    let mut wire = FetchResponse::synthetic(
        url(endpoint),
        200,
        Some("text/plain"),
        b"ok".to_vec(),
    );
    wire.headers
        .append_raw("set-cookie", "session=secret; Path=/");
    wire.headers.append_raw("set-cookie2", "legacy=secret");
    wire.headers.append_raw("x-visible", "yes");
    transport.respond(endpoint, wire);

    assert_eq!(browser.tick().requests_sent, 1);
    assert_eq!(transport.complete_all(), 1);
    browser.tick();

    assert_eq!(
        browser.document().runtime.console,
        vec![
            "cookie:false",
            "cookie2:false",
            "visible:yes",
            "body:ok",
        ]
    );
}
