use std::rc::Rc;

use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchRequest, FetchResponse, ManualNetwork, NetworkBackend, Url};
use browser_engine::SessionNetwork;

fn url(input: &str) -> Url {
    Url::parse(input).unwrap()
}

#[test]
fn hsts_upgrade_happens_before_secure_cookie_selection() {
    let clock = Rc::new(ManualClock::new());
    let transport = Rc::new(ManualNetwork::new());
    let session = SessionNetwork::with_new_state(transport.clone(), clock);

    session.cookie_jar().borrow_mut().store_set_cookie(
        "sid=secret; Secure; Path=/",
        &url("https://example.test/bootstrap"),
        0,
    );
    session.hsts_cache().borrow_mut().observe_response(
        &url("https://example.test/bootstrap"),
        "max-age=60",
        0,
    );

    session.start(1, FetchRequest::get(url("http://example.test/account")));

    let seen = transport.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].url.to_string(), "https://example.test/account");
    assert_eq!(seen[0].headers.get("cookie").as_deref(), Some("sid=secret"));
}

#[test]
fn one_secure_response_updates_hsts_and_cookie_session_state() {
    let clock = Rc::new(ManualClock::new());
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);

    let mut response = FetchResponse::synthetic(
        url("https://example.test/bootstrap"),
        200,
        Some("text/plain"),
        b"ok".to_vec(),
    );
    response
        .headers
        .append_raw("strict-transport-security", "max-age=60");
    response
        .headers
        .append_raw("set-cookie", "sid=learned; Secure; Path=/");
    transport.respond("https://example.test/bootstrap", response);

    let session = SessionNetwork::with_new_state(transport.clone(), clock);
    session.start(
        1,
        FetchRequest::get(url("https://example.test/bootstrap")),
    );
    let completed = session.poll();

    assert_eq!(completed.len(), 1);
    assert!(!completed[0]
        .result
        .as_ref()
        .unwrap()
        .headers
        .has("set-cookie"));
    assert!(session
        .hsts_cache()
        .borrow()
        .is_known_host("example.test", 0));

    session.start(2, FetchRequest::get(url("http://example.test/next")));
    let seen = transport.requests();
    assert_eq!(seen.last().unwrap().url.to_string(), "https://example.test/next");
    assert_eq!(
        seen.last().unwrap().headers.get("cookie").as_deref(),
        Some("sid=learned")
    );
}

#[test]
fn fresh_session_exposes_the_live_policy_state_handles() {
    let clock = Rc::new(ManualClock::new());
    let transport = Rc::new(ManualNetwork::new());
    let session = SessionNetwork::with_new_state(transport, clock);

    assert!(session.cookie_policy_registry().is_empty());
    assert_eq!(
        session
            .cookie_jar()
            .borrow()
            .get_http_cookie_header(&url("https://example.test/"), 0),
        None
    );
    assert!(!session
        .hsts_cache()
        .borrow()
        .is_known_host("example.test", 0));
}
