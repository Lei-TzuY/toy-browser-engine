use std::rc::Rc;
use std::time::Duration;

use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchResponse, ManualNetwork, MemoryLoader, Url};
use browser_engine::script::dom_api;
use browser_engine::Browser;

fn text(browser: &Browser, id: &str) -> String {
    let path = dom_api::get_element_by_id(&browser.document().dom, id).expect("element exists");
    dom_api::text_content(dom_api::node_at(&browser.document().dom, &path).unwrap())
}

fn response_with_cookie(url: &str, cookie: &str, body: &str) -> FetchResponse {
    let parsed = Url::parse(url).unwrap();
    let mut response = FetchResponse::synthetic(
        parsed,
        200,
        Some("text/plain; charset=utf-8"),
        body.as_bytes().to_vec(),
    );
    response.headers.append_raw("set-cookie", cookie);
    response
}

#[test]
fn document_cookie_survives_navigation_and_is_visible_during_next_bootstrap() {
    let mut loader = MemoryLoader::new();
    loader.insert(
        "http://example.test/index.html",
        r#"
            <a id="next" href="/next.html">next</a>
            <script>document.cookie = "session=abc; Path=/";</script>
        "#,
    );
    loader.insert(
        "http://example.test/next.html",
        r#"
            <p id="seen"></p>
            <script>document.getElementById("seen").textContent = document.cookie;</script>
        "#,
    );

    let mut browser = Browser::open(
        Box::new(loader),
        &Url::parse("http://example.test/index.html").unwrap(),
    )
    .unwrap();
    let jar = browser.cookie_jar();
    assert!(Rc::ptr_eq(&browser.document().runtime.cookie_jar, &jar));

    browser.follow_link("/next.html").unwrap();
    assert_eq!(text(&browser, "seen"), "session=abc");
    assert!(Rc::ptr_eq(&browser.document().runtime.cookie_jar, &jar));
}

#[test]
fn reload_and_history_reloads_keep_the_same_cookie_session() {
    let mut loader = MemoryLoader::new();
    loader.insert(
        "http://example.test/a.html",
        r#"
            <p id="seen"></p>
            <a href="/b.html" id="b">b</a>
            <script>
                document.getElementById("seen").textContent = document.cookie;
                document.cookie = "persist=yes; Path=/";
            </script>
        "#,
    );
    loader.insert(
        "http://example.test/b.html",
        r#"
            <p id="seen"></p>
            <script>document.getElementById("seen").textContent = document.cookie;</script>
        "#,
    );

    let mut browser = Browser::open(
        Box::new(loader),
        &Url::parse("http://example.test/a.html").unwrap(),
    )
    .unwrap();
    assert_eq!(text(&browser, "seen"), "");

    browser.reload().unwrap();
    assert!(text(&browser, "seen").contains("persist=yes"));

    browser.follow_link("/b.html").unwrap();
    assert!(text(&browser, "seen").contains("persist=yes"));

    assert!(browser.back());
    assert!(text(&browser, "seen").contains("persist=yes"));

    assert!(browser.forward());
    assert!(text(&browser, "seen").contains("persist=yes"));
}

#[test]
fn fetch_set_cookie_updates_document_cookie_and_the_next_request() {
    let mut loader = MemoryLoader::new();
    loader.insert(
        "http://example.test/index.html",
        r#"
            <p id="seen">pending</p>
            <script>
                fetch("/login")
                  .then(function (response) { return response.text(); })
                  .then(function () {
                    document.getElementById("seen").textContent = document.cookie;
                    return fetch("/whoami");
                  })
                  .then(function (response) { return response.text(); })
                  .then(function (body) {
                    document.getElementById("seen").textContent =
                      document.getElementById("seen").textContent + "|" + body;
                  });
            </script>
        "#,
    );

    let manual = Rc::new(ManualNetwork::new());
    manual.set_auto_complete(true);
    manual.respond(
        "http://example.test/login",
        response_with_cookie(
            "http://example.test/login",
            "auth=token123; Path=/; HttpOnly",
            "logged in",
        ),
    );
    manual.respond_text("http://example.test/whoami", "who");

    let clock = Rc::new(ManualClock::new());
    let mut browser = Browser::open_with_network(
        Box::new(loader),
        manual.clone(),
        &Url::parse("http://example.test/index.html").unwrap(),
        clock,
    )
    .unwrap();

    browser.settle_network(16);

    // HttpOnly is deliberately invisible to document.cookie, but it still
    // participates in the next HTTP request.
    assert_eq!(text(&browser, "seen"), "|who");
    let requests = manual.requests();
    assert_eq!(requests.len(), 2, "login followed by whoami");
    assert!(requests[0].headers.get("cookie").is_none());
    assert_eq!(
        requests[1].headers.get("cookie"),
        Some("auth=token123".to_string())
    );
    assert!(browser
        .cookie_jar()
        .borrow()
        .get_http_cookie_header(&Url::parse("http://example.test/anything").unwrap(), 0,)
        .unwrap()
        .contains("auth=token123"));
}

#[test]
fn non_httponly_fetch_cookie_is_immediately_visible_to_script() {
    let mut loader = MemoryLoader::new();
    loader.insert(
        "http://example.test/index.html",
        r#"
            <p id="seen">pending</p>
            <script>
                fetch("/prefs")
                  .then(function () {
                    document.getElementById("seen").textContent = document.cookie;
                  });
            </script>
        "#,
    );

    let manual = Rc::new(ManualNetwork::new());
    manual.set_auto_complete(true);
    manual.respond(
        "http://example.test/prefs",
        response_with_cookie("http://example.test/prefs", "theme=dark; Path=/", "ok"),
    );

    let mut browser = Browser::open_with_network(
        Box::new(loader),
        manual,
        &Url::parse("http://example.test/index.html").unwrap(),
        Rc::new(ManualClock::new()),
    )
    .unwrap();
    browser.settle_network(8);
    assert_eq!(text(&browser, "seen"), "theme=dark");
}

#[test]
fn max_age_uses_session_clock_after_document_time_resets() {
    let mut loader = MemoryLoader::new();
    loader.insert(
        "http://example.test/a.html",
        r#"
            <script>document.cookie = "short=1; Path=/; Max-Age=1";</script>
        "#,
    );
    loader.insert(
        "http://example.test/b.html",
        r#"
            <p id="seen"></p>
            <script>document.getElementById("seen").textContent = document.cookie;</script>
        "#,
    );

    let clock = Rc::new(ManualClock::new());
    let mut browser = Browser::open_with_clock(
        Box::new(loader),
        &Url::parse("http://example.test/a.html").unwrap(),
        clock,
    )
    .unwrap();

    browser.advance_time(Duration::from_millis(500));
    browser.follow_link("/b.html").unwrap();
    assert_eq!(text(&browser, "seen"), "short=1");

    // The new Document's runtime time is now only 600ms, but session time is
    // 1100ms. The bound CookieJar must use the latter.
    browser.advance_time(Duration::from_millis(600));
    {
        let document = browser.document_mut();
        document.runtime.run_script(
            &mut document.dom,
            r#"document.getElementById("seen").textContent = document.cookie;"#,
        );
    }
    assert_eq!(text(&browser, "seen"), "");
}
