use std::rc::Rc;

use browser_engine::browser::Browser;
use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Url};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn response_with_cookie(url_text: &str, cookie: &str) -> FetchResponse {
    let mut response =
        FetchResponse::synthetic(url(url_text), 200, Some("text/plain"), b"ok".to_vec());
    response.headers.append_raw("set-cookie", cookie);
    response
}

fn browser_with_script(
    script: &str,
    endpoint: &str,
    response_cookie: &str,
) -> (Browser, Rc<ManualNetwork>) {
    let page = "http://example.test/index.html";
    let mut loader = MemoryLoader::new();
    loader.insert(page, format!("<script>{script}</script>"));

    let transport = Rc::new(ManualNetwork::new());
    transport.respond(endpoint, response_with_cookie(endpoint, response_cookie));

    let browser = Browser::open_with_network(
        Box::new(loader),
        transport.clone(),
        &url(page),
        Rc::new(ManualClock::new()),
    )
    .expect("page loads");
    (browser, transport)
}

#[test]
fn fetch_credentials_omit_sends_no_cookie_and_ignores_set_cookie() {
    let endpoint = "http://example.test/api";
    let (mut browser, transport) = browser_with_script(
        r#"fetch("/api", { credentials: "omit" })
               .then(function () { console.log("omit done"); });"#,
        endpoint,
        "server=new; Path=/",
    );

    let jar = browser.cookie_jar();
    assert!(jar.borrow_mut().store_set_cookie(
        "session=old; Path=/",
        &url("http://example.test/"),
        0,
    ));

    let first = browser.tick();
    assert_eq!(first.requests_sent, 1);
    let sent = transport.requests();
    assert_eq!(sent.len(), 1);
    assert!(
        sent[0].headers.get("cookie").is_none(),
        "credentials=omit must suppress the session Cookie header"
    );

    assert_eq!(transport.complete_all(), 1);
    let second = browser.tick();
    assert_eq!(second.network_completions, 1);
    assert_eq!(browser.document().runtime.console, vec!["omit done"]);
    assert_eq!(
        jar.borrow()
            .get_http_cookie_header(&url(endpoint), 0)
            .as_deref(),
        Some("session=old"),
        "credentials=omit must ignore response Set-Cookie"
    );
}

#[test]
fn default_fetch_keeps_same_origin_cookie_send_and_store_behavior() {
    let endpoint = "http://example.test/api";
    let (mut browser, transport) = browser_with_script(
        r#"fetch("/api").then(function () { console.log("default done"); });"#,
        endpoint,
        "server=new; Path=/",
    );

    let jar = browser.cookie_jar();
    assert!(jar.borrow_mut().store_set_cookie(
        "session=old; Path=/",
        &url("http://example.test/"),
        0,
    ));

    browser.tick();
    assert_eq!(
        transport.requests()[0].headers.get("cookie").as_deref(),
        Some("session=old")
    );

    assert_eq!(transport.complete_all(), 1);
    browser.tick();
    assert_eq!(browser.document().runtime.console, vec!["default done"]);
    assert_eq!(
        jar.borrow()
            .get_http_cookie_header(&url(endpoint), 0)
            .as_deref(),
        Some("session=old; server=new"),
        "default same-origin fetch keeps participating in cookie state"
    );
}

#[test]
fn explicit_same_origin_credentials_match_the_default() {
    let endpoint = "http://example.test/api";
    let (mut browser, transport) = browser_with_script(
        r#"fetch("/api", { credentials: "same-origin" });"#,
        endpoint,
        "server=new; Path=/",
    );

    let jar = browser.cookie_jar();
    assert!(jar.borrow_mut().store_set_cookie(
        "session=old; Path=/",
        &url("http://example.test/"),
        0,
    ));

    browser.tick();
    assert_eq!(
        transport.requests()[0].headers.get("cookie").as_deref(),
        Some("session=old")
    );
}

#[test]
fn omit_policy_is_isolated_between_concurrent_fetch_ids() {
    let page = "http://example.test/index.html";
    let mut loader = MemoryLoader::new();
    loader.insert(
        page,
        r#"<script>
              fetch("/omit", { credentials: "omit" });
              fetch("/include");
            </script>"#,
    );

    let transport = Rc::new(ManualNetwork::new());
    let mut browser = Browser::open_with_network(
        Box::new(loader),
        transport.clone(),
        &url(page),
        Rc::new(ManualClock::new()),
    )
    .expect("page loads");

    let jar = browser.cookie_jar();
    assert!(jar.borrow_mut().store_set_cookie(
        "session=old; Path=/",
        &url("http://example.test/"),
        0,
    ));

    let report = browser.tick();
    assert_eq!(report.requests_sent, 2);
    let sent = transport.requests();
    assert_eq!(sent.len(), 2);
    assert!(sent[0].url.to_string().ends_with("/omit"));
    assert!(sent[0].headers.get("cookie").is_none());
    assert!(sent[1].url.to_string().ends_with("/include"));
    assert_eq!(
        sent[1].headers.get("cookie").as_deref(),
        Some("session=old")
    );
}
