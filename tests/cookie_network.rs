use std::cell::RefCell;
use std::rc::Rc;

use browser_engine::cookie::CookieJar;
use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchRequest, FetchResponse, ManualNetwork, NetworkBackend, Url};
use browser_engine::CookieNetwork;

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn response_with_cookies(url_text: &str, cookies: &[&str]) -> FetchResponse {
    let mut response = FetchResponse::synthetic(
        url(url_text),
        200,
        Some("text/plain; charset=utf-8"),
        b"ok".to_vec(),
    );
    for cookie in cookies {
        response.headers.append_raw("set-cookie", cookie);
    }
    response
}

#[test]
fn host_only_and_domain_cookies_have_distinct_scope() {
    let mut jar = CookieJar::new();
    let origin = url("https://www.example.test/account/login");

    let host_only = CookieJar::parse_set_cookie("host=1; Path=/", &origin, 1_000).unwrap();
    assert!(host_only.host_only);
    jar.store(host_only, 1_000);

    assert_eq!(
        jar.get_http_cookie_header(&url("https://www.example.test/next"), 1_000),
        Some("host=1".to_string())
    );
    assert!(jar
        .get_http_cookie_header(&url("https://api.example.test/next"), 1_000)
        .is_none());

    let domain =
        CookieJar::parse_set_cookie("shared=1; Domain=example.test; Path=/", &origin, 1_000)
            .unwrap();
    assert!(!domain.host_only);
    jar.store(domain, 1_000);

    assert_eq!(
        jar.get_http_cookie_header(&url("https://api.example.test/next"), 1_000),
        Some("shared=1".to_string())
    );
}

#[test]
fn unrelated_domain_and_non_http_cookie_sources_are_rejected() {
    let origin = url("https://shop.example.test/cart");
    assert!(
        CookieJar::parse_set_cookie("poison=1; Domain=attacker.test; Path=/", &origin, 0).is_none()
    );
    assert!(
        CookieJar::parse_set_cookie("poison=1; Domain=ample.test; Path=/", &origin, 0).is_none()
    );

    assert!(
        CookieJar::parse_set_cookie("local=1; Path=/", &url("file:///tmp/index.html"), 0).is_none()
    );
    assert!(CookieJar::parse_set_cookie("demo=1; Path=/", &url("demo:///index.html"), 0).is_none());
}

#[test]
fn response_cookie_is_stored_and_sent_on_the_next_request() {
    let inner = Rc::new(ManualNetwork::new());
    inner.set_auto_complete(true);
    inner.respond(
        "http://example.test/login",
        response_with_cookies(
            "http://example.test/login",
            &["session=abc; Path=/; HttpOnly", "theme=dark; Path=/app"],
        ),
    );

    let jar = Rc::new(RefCell::new(CookieJar::new()));
    let clock = Rc::new(ManualClock::starting_at(10_000.0));
    let network = CookieNetwork::new(inner.clone(), jar.clone(), clock);

    network.start(1, FetchRequest::get(url("http://example.test/login")));
    let completions = network.poll();
    assert_eq!(completions.len(), 1);
    let response = completions[0].result.as_ref().expect("response succeeds");
    assert!(
        response.headers.get("set-cookie").is_none(),
        "Fetch must not expose Set-Cookie to page script"
    );
    assert_eq!(jar.borrow().len(), 2);

    network.start(2, FetchRequest::get(url("http://example.test/app/data")));
    network.poll();
    let requests = inner.requests();
    assert_eq!(requests.len(), 2);
    let cookie = requests[1].headers.get("cookie").expect("cookie header");
    assert!(cookie.contains("session=abc"));
    assert!(cookie.contains("theme=dark"));
}

#[test]
fn browser_owned_cookie_header_replaces_page_supplied_value() {
    let inner = Rc::new(ManualNetwork::new());
    inner.set_auto_complete(true);
    let jar = Rc::new(RefCell::new(CookieJar::new()));
    let origin = url("http://example.test/index.html");
    let cookie = CookieJar::parse_set_cookie("trusted=yes; Path=/", &origin, 0).unwrap();
    jar.borrow_mut().store(cookie, 0);

    let network = CookieNetwork::new(inner.clone(), jar, Rc::new(ManualClock::new()));
    let mut request = FetchRequest::get(url("http://example.test/api"));
    request.headers.insert_raw("cookie", "forged=evil");
    network.start(9, request);

    let sent = inner.requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].headers.get("cookie"),
        Some("trusted=yes".to_string())
    );
}

#[test]
fn secure_and_expired_cookies_are_not_sent() {
    let inner = Rc::new(ManualNetwork::new());
    inner.set_auto_complete(true);
    let jar = Rc::new(RefCell::new(CookieJar::new()));
    let clock = Rc::new(ManualClock::starting_at(5_000.0));
    let network = CookieNetwork::new(inner.clone(), jar.clone(), clock.clone());

    let secure = CookieJar::parse_set_cookie(
        "secure_token=1; Path=/; Secure; Max-Age=1",
        &url("https://example.test/login"),
        5_000,
    )
    .unwrap();
    jar.borrow_mut().store(secure, 5_000);

    network.start(1, FetchRequest::get(url("http://example.test/plain")));
    assert!(inner.requests()[0].headers.get("cookie").is_none());

    network.start(2, FetchRequest::get(url("https://example.test/secure")));
    assert_eq!(
        inner.requests()[1].headers.get("cookie"),
        Some("secure_token=1".to_string())
    );

    clock.set(6_001.0);
    network.start(3, FetchRequest::get(url("https://example.test/expired")));
    assert!(inner.requests()[2].headers.get("cookie").is_none());
}

#[test]
fn multiple_set_cookie_lines_are_processed_independently() {
    let inner = Rc::new(ManualNetwork::new());
    inner.set_auto_complete(true);
    inner.respond(
        "https://www.example.test/login",
        response_with_cookies(
            "https://www.example.test/login",
            &[
                "host=1; Path=/",
                "shared=2; Domain=example.test; Path=/",
                "rejected=3; Domain=not-example.test; Path=/",
            ],
        ),
    );
    let jar = Rc::new(RefCell::new(CookieJar::new()));
    let network = CookieNetwork::new(inner.clone(), jar.clone(), Rc::new(ManualClock::new()));

    network.start(1, FetchRequest::get(url("https://www.example.test/login")));
    network.poll();
    assert_eq!(
        jar.borrow().len(),
        2,
        "the unrelated Domain cookie is rejected"
    );

    network.start(2, FetchRequest::get(url("https://api.example.test/data")));
    let sent = inner.requests();
    assert_eq!(
        sent[1].headers.get("cookie"),
        Some("shared=2".to_string()),
        "host-only cookie stays on www.example.test"
    );
}
