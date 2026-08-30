use std::rc::Rc;

use browser_engine::eventloop::ManualClock;
use browser_engine::net::{
    FetchError, FetchRequest, FetchResponse, ManualNetwork, NetworkBackend, Url,
};
use browser_engine::SessionNetwork;

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

#[test]
fn redirecting_session_rejects_a_transport_that_already_followed_redirects() {
    let clock = Rc::new(ManualClock::new());
    let transport = Rc::new(ManualNetwork::new());
    let network = SessionNetwork::with_new_state_redirecting(transport.clone(), clock);

    let mut response = FetchResponse::synthetic(
        url("http://example.test/final"),
        200,
        Some("text/plain"),
        b"final".to_vec(),
    );
    response.redirected = true;
    response
        .headers
        .append_raw("set-cookie", "should_not_store=1; Path=/");
    response
        .headers
        .append_raw("strict-transport-security", "max-age=3600");
    transport.respond("http://example.test/start", response);

    network.start(41, FetchRequest::get(url("http://example.test/start")));
    assert!(transport.complete(41));

    let completions = network.poll();
    assert_eq!(completions.len(), 1);
    assert!(matches!(
        completions[0].result,
        Err(FetchError::MalformedResponse(_))
    ));

    let origin = url("http://example.test/");
    assert_eq!(
        network
            .cookie_jar()
            .borrow()
            .get_document_cookie(&origin, 0),
        "",
        "a policy-skipping pre-followed response must not mutate the cookie jar"
    );
    assert_eq!(
        network
            .hsts_cache()
            .borrow()
            .upgrade_url(&url("http://example.test/next"), 0)
            .to_string(),
        "http://example.test/next",
        "a policy-skipping pre-followed response must not teach HSTS"
    );
}

#[test]
fn redirecting_session_still_accepts_an_ordinary_single_hop_response() {
    let clock = Rc::new(ManualClock::new());
    let transport = Rc::new(ManualNetwork::new());
    let network = SessionNetwork::with_new_state_redirecting(transport.clone(), clock);

    transport.respond_text("http://example.test/data", "ok");
    network.start(42, FetchRequest::get(url("http://example.test/data")));
    assert!(transport.complete(42));

    let completions = network.poll();
    assert_eq!(completions.len(), 1);
    let response = completions[0].result.as_ref().expect("successful response");
    assert_eq!(response.status, 200);
    assert!(!response.redirected);
}
