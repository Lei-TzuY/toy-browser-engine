use std::rc::Rc;

use browser_engine::browser::Browser;
use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Url};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn secure_policy_response() -> FetchResponse {
    let mut response = FetchResponse::synthetic(
        url("https://example.test/learn"),
        200,
        Some("text/plain"),
        b"learned".to_vec(),
    );
    response
        .headers
        .append_raw("strict-transport-security", "max-age=3600");
    response
        .headers
        .append_raw("set-cookie", "auth=secure; Path=/; Secure");
    response
}

#[test]
fn browser_fetch_uses_hsts_before_secure_cookie_selection() {
    let page = "http://example.test/index.html";
    let mut loader = MemoryLoader::new();
    loader.insert(
        page,
        r#"
            <script>
                fetch("/learn")
                  .then(function () { return fetch("/after"); })
                  .then(function () { console.log("done"); });
            </script>
        "#,
    );

    let transport = Rc::new(ManualNetwork::new());
    transport.respond("http://example.test/learn", secure_policy_response());
    transport.respond_text("https://example.test/after", "after");

    let mut browser = Browser::open_with_network(
        Box::new(loader),
        transport.clone(),
        &url(page),
        Rc::new(ManualClock::new()),
    )
    .expect("page loads");

    // First turn dispatches the authored HTTP request. No HSTS state exists yet.
    let first = browser.tick();
    assert_eq!(first.requests_sent, 1);
    let sent = transport.requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].url.to_string(), "http://example.test/learn");
    assert!(sent[0].headers.get("cookie").is_none());

    // Deliver a final HTTPS response. CookieNetwork stores its Secure cookie;
    // HstsNetwork then learns the STS policy from that same completion.
    assert_eq!(transport.complete_all(), 1);
    let completion = browser.tick();
    assert_eq!(completion.network_completions, 1);

    // The promise reaction queued /after. A later network phase dispatches it.
    for _ in 0..3 {
        if transport.requests().len() >= 2 {
            break;
        }
        browser.tick();
    }

    let sent = transport.requests();
    assert_eq!(sent.len(), 2, "the chained fetch should reach transport");
    assert_eq!(
        sent[1].url.to_string(),
        "https://example.test/after",
        "Browser must route Fetch through HSTS before transport"
    );
    assert_eq!(
        sent[1].headers.get("cookie").as_deref(),
        Some("auth=secure"),
        "Secure-cookie selection must observe the HSTS-upgraded HTTPS URL"
    );
}

#[test]
fn browser_session_network_keeps_existing_credentials_omit_policy() {
    let page = "http://example.test/index.html";
    let mut loader = MemoryLoader::new();
    loader.insert(
        page,
        r#"<script>fetch("/api", { credentials: "omit" });</script>"#,
    );

    let transport = Rc::new(ManualNetwork::new());
    let mut browser = Browser::open_with_network(
        Box::new(loader),
        transport.clone(),
        &url(page),
        Rc::new(ManualClock::new()),
    )
    .expect("page loads");

    assert!(browser.cookie_jar().borrow_mut().store_set_cookie(
        "session=old; Path=/",
        &url("http://example.test/"),
        0,
    ));

    browser.tick();
    let sent = transport.requests();
    assert_eq!(sent.len(), 1);
    assert!(
        sent[0].headers.get("cookie").is_none(),
        "SessionNetwork must preserve CookieNetwork's per-FetchId omit registry"
    );
}
