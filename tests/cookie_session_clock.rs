use std::rc::Rc;

use browser_engine::cookie::CookieJar;
use browser_engine::eventloop::ManualClock;
use browser_engine::Url;

fn url() -> Url {
    Url::parse("https://example.test/app/page.html").unwrap()
}

#[test]
fn bound_clock_wins_over_document_relative_fallback_time() {
    let clock = Rc::new(ManualClock::starting_at(10_000.0));
    let mut jar = CookieJar::with_clock(clock.clone());
    let page = url();

    // A page-relative runtime would currently report 0ms here. The bound
    // session clock is 10s, so Max-Age=2 expires at session time 12s.
    jar.set_document_cookie("session=alive; Path=/; Max-Age=2", &page, 0);
    assert_eq!(
        jar.get_document_cookie(&page, 0),
        "session=alive",
        "document-relative time must not replace the session clock"
    );

    // Simulate navigation: the new document's runtime clock reset to zero,
    // but the browser/session clock kept moving.
    clock.set(11_500.0);
    assert_eq!(jar.get_document_cookie(&page, 0), "session=alive");

    clock.set(12_001.0);
    assert_eq!(
        jar.get_document_cookie(&page, 0),
        "",
        "navigation must not extend a Max-Age cookie"
    );
}

#[test]
fn bound_clock_also_controls_http_cookie_headers() {
    let clock = Rc::new(ManualClock::starting_at(5_000.0));
    let mut jar = CookieJar::with_clock(clock.clone());
    let page = url();

    jar.set_document_cookie("api=1; Path=/; Max-Age=1", &page, 999_999);
    assert_eq!(
        jar.get_http_cookie_header(&page, 0),
        Some("api=1".to_string())
    );

    clock.set(6_001.0);
    assert!(
        jar.get_http_cookie_header(&page, 0).is_none(),
        "network lookups and document.cookie must share one time domain"
    );
}

#[test]
fn standalone_jar_keeps_explicit_manual_time_semantics() {
    let mut jar = CookieJar::new();
    let page = url();

    jar.set_document_cookie("standalone=1; Path=/; Max-Age=1", &page, 7_000);
    assert_eq!(jar.get_document_cookie(&page, 7_999), "standalone=1");
    assert_eq!(jar.get_document_cookie(&page, 8_000), "");
}

#[test]
fn binding_a_clock_preserves_existing_cookie_state() {
    let mut jar = CookieJar::new();
    let page = url();
    jar.set_document_cookie("persist=1; Path=/", &page, 0);

    let clock = Rc::new(ManualClock::starting_at(50_000.0));
    jar.bind_clock(clock);
    assert!(jar.has_bound_clock());
    assert_eq!(jar.get_document_cookie(&page, 0), "persist=1");
}

#[test]
fn cloned_session_jar_keeps_the_same_clock_source() {
    let clock = Rc::new(ManualClock::starting_at(1_000.0));
    let mut jar = CookieJar::with_clock(clock.clone());
    let page = url();
    jar.set_document_cookie("short=1; Path=/; Max-Age=1", &page, 0);

    let cloned = jar.clone();
    assert_eq!(jar, cloned, "clock identity is not cookie data");
    clock.set(2_001.0);
    assert_eq!(cloned.get_document_cookie(&page, 0), "");
}
