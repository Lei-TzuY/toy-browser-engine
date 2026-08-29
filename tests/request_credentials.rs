use std::rc::Rc;

use browser_engine::browser::Browser;
use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Url};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn response_with_cookie(url_text: &str, cookie: &str) -> FetchResponse {
    let mut response = FetchResponse::synthetic(
        url(url_text),
        200,
        Some("text/plain"),
        b"ok".to_vec(),
    );
    response.headers.append_raw("set-cookie", cookie);
    response
}

fn browser_with_script(script: &str) -> (Browser, Rc<ManualNetwork>) {
    let page = "http://example.test/index.html";
    let mut loader = MemoryLoader::new();
    loader.insert(page, format!("<script>{script}</script>"));

    let transport = Rc::new(ManualNetwork::new());
    transport.respond(
        "http://example.test/api",
        response_with_cookie(
            "http://example.test/api",
            "server=new; Path=/",
        ),
    );

    let browser = Browser::open_with_network(
        Box::new(loader),
        transport.clone(),
        &url(page),
        Rc::new(ManualClock::new()),
    )
    .expect("page loads");
    (browser, transport)
}

fn seed_session_cookie(browser: &Browser) {
    assert!(browser.cookie_jar().borrow_mut().store_set_cookie(
        "session=old; Path=/",
        &url("http://example.test/"),
        0,
    ));
}

#[test]
fn request_credentials_omit_is_script_visible_and_used_by_fetch_request() {
    let (mut browser, transport) = browser_with_script(
        r#"
            var request = new Request("/api", { credentials: "omit" });
            console.log(request.credentials);
            fetch(request).then(function () { console.log("done"); });
        "#,
    );
    seed_session_cookie(&browser);

    let first = browser.tick();
    assert_eq!(first.requests_sent, 1);
    assert_eq!(browser.document().runtime.console, vec!["omit"]);
    assert!(transport.requests()[0].headers.get("cookie").is_none());

    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(browser.document().runtime.console, vec!["omit", "done"]);
    assert_eq!(
        browser
            .cookie_jar()
            .borrow()
            .get_http_cookie_header(&url("http://example.test/api"), 0)
            .as_deref(),
        Some("session=old"),
        "the Request's omit mode must also reject response Set-Cookie"
    );
}

#[test]
fn cloning_a_request_inherits_credentials() {
    let (mut browser, transport) = browser_with_script(
        r#"
            var original = new Request("/api", { credentials: "omit" });
            var clone = new Request(original);
            console.log(clone.credentials);
            fetch(clone);
        "#,
    );
    seed_session_cookie(&browser);

    browser.tick();
    assert_eq!(browser.document().runtime.console, vec!["omit"]);
    assert!(
        transport.requests()[0].headers.get("cookie").is_none(),
        "new Request(existing) must retain the existing omit mode"
    );
}

#[test]
fn request_constructor_init_overrides_inherited_credentials() {
    let (mut browser, transport) = browser_with_script(
        r#"
            var original = new Request("/api", { credentials: "omit" });
            var clone = new Request(original, { credentials: "same-origin" });
            console.log(clone.credentials);
            fetch(clone);
        "#,
    );
    seed_session_cookie(&browser);

    browser.tick();
    assert_eq!(browser.document().runtime.console, vec!["same-origin"]);
    assert_eq!(
        transport.requests()[0].headers.get("cookie").as_deref(),
        Some("session=old"),
        "constructor init must override the source Request's credentials"
    );
}

#[test]
fn fetch_init_overrides_request_credentials_without_mutating_the_request() {
    let (mut browser, transport) = browser_with_script(
        r#"
            var request = new Request("/api", { credentials: "omit" });
            fetch(request, { credentials: "same-origin" });
            console.log(request.credentials);
        "#,
    );
    seed_session_cookie(&browser);

    browser.tick();
    assert_eq!(browser.document().runtime.console, vec!["omit"]);
    assert_eq!(
        transport.requests()[0].headers.get("cookie").as_deref(),
        Some("session=old"),
        "fetch-level init must override the copied request used for this fetch"
    );
}

#[test]
fn new_request_defaults_to_same_origin_credentials() {
    let (browser, _transport) = browser_with_script(
        r#"
            var request = new Request("/api");
            console.log(request.credentials);
        "#,
    );
    assert_eq!(browser.document().runtime.console, vec!["same-origin"]);
}
