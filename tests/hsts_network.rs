use std::rc::Rc;

use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchError, FetchRequest, FetchResponse, ManualNetwork, NetworkBackend, Url};
use browser_engine::HstsNetwork;

fn url(input: &str) -> Url {
    Url::parse(input).unwrap()
}

#[test]
fn secure_response_learns_hsts_and_later_http_request_is_upgraded() {
    let clock = Rc::new(ManualClock::new());
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);

    let mut learned = FetchResponse::synthetic(
        url("https://example.test/bootstrap"),
        200,
        Some("text/plain"),
        b"secure".to_vec(),
    );
    learned
        .headers
        .append_raw("strict-transport-security", "max-age=60; includeSubDomains");
    transport.respond("https://example.test/bootstrap", learned);
    transport.respond_text("https://api.example.test/data", "upgraded");

    let network = HstsNetwork::with_new_cache(transport.clone(), clock);
    network.start(1, FetchRequest::get(url("https://example.test/bootstrap")));
    let first = network.poll();
    assert_eq!(first.len(), 1);
    assert!(network.cache().borrow().is_known_host("example.test", 0));

    network.start(2, FetchRequest::get(url("http://api.example.test/data")));
    let seen = transport.requests();
    assert_eq!(seen.last().unwrap().url.to_string(), "https://api.example.test/data");

    let second = network.poll();
    assert_eq!(second.len(), 1);
    assert_eq!(
        second[0].result.as_ref().unwrap().url.to_string(),
        "https://api.example.test/data"
    );
}

#[test]
fn upgrade_applies_port_mapping_before_transport_sees_request() {
    let clock = Rc::new(ManualClock::new());
    let transport = Rc::new(ManualNetwork::new());
    let network = HstsNetwork::with_new_cache(transport.clone(), clock);

    network.cache().borrow_mut().observe_response(
        &url("https://example.test/"),
        "max-age=60",
        0,
    );

    network.start(1, FetchRequest::get(url("http://example.test:80/a?q=1#frag")));
    network.start(2, FetchRequest::get(url("http://example.test:8080/b")));

    let seen = transport.requests();
    assert_eq!(seen[0].url.to_string(), "https://example.test:443/a?q=1#frag");
    assert_eq!(seen[1].url.to_string(), "https://example.test:8080/b");
}

#[test]
fn expired_policy_stops_upgrading_without_a_separate_purge_step() {
    let clock = Rc::new(ManualClock::new());
    let transport = Rc::new(ManualNetwork::new());
    let network = HstsNetwork::with_new_cache(transport.clone(), clock.clone());

    network.cache().borrow_mut().observe_response(
        &url("https://example.test/"),
        "max-age=1",
        0,
    );
    clock.set(1_000.0);

    network.start(1, FetchRequest::get(url("http://example.test/plain")));
    assert_eq!(
        transport.requests()[0].url.to_string(),
        "http://example.test/plain"
    );
}

#[test]
fn insecure_duplicate_and_failed_responses_cannot_establish_policy() {
    let clock = Rc::new(ManualClock::new());
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);

    let mut insecure = FetchResponse::synthetic(
        url("http://example.test/insecure"),
        200,
        Some("text/plain"),
        Vec::new(),
    );
    insecure
        .headers
        .append_raw("strict-transport-security", "max-age=60");
    transport.respond("http://example.test/insecure", insecure);

    let mut duplicate = FetchResponse::synthetic(
        url("https://duplicate.test/"),
        200,
        Some("text/plain"),
        Vec::new(),
    );
    duplicate
        .headers
        .append_raw("strict-transport-security", "max-age=0");
    duplicate
        .headers
        .append_raw("strict-transport-security", "max-age=60");
    transport.respond("https://duplicate.test/", duplicate);
    transport.fail(
        "https://failed.test/",
        FetchError::Io("transport failed".into()),
    );

    let network = HstsNetwork::with_new_cache(transport, clock);
    network.start(1, FetchRequest::get(url("http://example.test/insecure")));
    network.start(2, FetchRequest::get(url("https://duplicate.test/")));
    network.start(3, FetchRequest::get(url("https://failed.test/")));
    let completions = network.poll();
    assert_eq!(completions.len(), 3);

    let cache = network.cache();
    let cache = cache.borrow();
    assert!(!cache.is_known_host("example.test", 0));
    assert!(!cache.is_known_host("duplicate.test", 0));
    assert!(!cache.is_known_host("failed.test", 0));
}

#[test]
fn cancellation_is_delegated_to_the_wrapped_backend() {
    let clock = Rc::new(ManualClock::new());
    let transport = Rc::new(ManualNetwork::new());
    let network = HstsNetwork::with_new_cache(transport.clone(), clock);

    network.start(7, FetchRequest::get(url("http://example.test/slow")));
    assert_eq!(transport.pending_count(), 1);
    network.cancel(7);
    assert_eq!(transport.pending_count(), 0);
    assert!(network.poll().is_empty());
}
