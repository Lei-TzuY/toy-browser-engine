use std::rc::Rc;

use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchRequest, FetchResponse, ManualNetwork, NetworkBackend, Url};
use browser_engine::HstsNetwork;

fn url(input: &str) -> Url {
    Url::parse(input).unwrap()
}

#[test]
fn malformed_extension_directive_prevents_hsts_learning() {
    let clock = Rc::new(ManualClock::new());
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);

    let mut bootstrap = FetchResponse::synthetic(
        url("https://example.test/bootstrap"),
        200,
        Some("text/plain"),
        Vec::new(),
    );
    bootstrap
        .headers
        .append_raw("strict-transport-security", "max-age=60; bad directive");
    transport.respond("https://example.test/bootstrap", bootstrap);
    transport.respond_text("http://example.test/plain", "plain");

    let network = HstsNetwork::with_new_cache(transport.clone(), clock);
    network.start(1, FetchRequest::get(url("https://example.test/bootstrap")));
    assert_eq!(network.poll().len(), 1);
    assert!(!network.cache().borrow().is_known_host("example.test", 0));

    network.start(2, FetchRequest::get(url("http://example.test/plain")));
    assert_eq!(
        transport.requests().last().unwrap().url.to_string(),
        "http://example.test/plain"
    );
}

#[test]
fn valid_unknown_extension_still_allows_hsts_learning() {
    let clock = Rc::new(ManualClock::new());
    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);

    let mut bootstrap = FetchResponse::synthetic(
        url("https://example.test/bootstrap"),
        200,
        Some("text/plain"),
        Vec::new(),
    );
    bootstrap.headers.append_raw(
        "strict-transport-security",
        "max-age=60; ext=\"quoted value\"; includeSubDomains",
    );
    transport.respond("https://example.test/bootstrap", bootstrap);
    transport.respond_text("https://api.example.test/data", "upgraded");

    let network = HstsNetwork::with_new_cache(transport.clone(), clock);
    network.start(1, FetchRequest::get(url("https://example.test/bootstrap")));
    assert_eq!(network.poll().len(), 1);
    assert!(network.cache().borrow().is_known_host("api.example.test", 0));

    network.start(2, FetchRequest::get(url("http://api.example.test/data")));
    assert_eq!(
        transport.requests().last().unwrap().url.to_string(),
        "https://api.example.test/data"
    );
}
