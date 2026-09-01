use std::rc::Rc;

use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchRequest, ManualNetwork, NetworkBackend, Url};
use browser_engine::HstsNetwork;

fn url(input: &str) -> Url {
    Url::parse(input).unwrap()
}

#[test]
fn expired_hsts_policy_is_evicted_on_the_next_network_request() {
    let clock = Rc::new(ManualClock::new());
    let transport = Rc::new(ManualNetwork::new());
    let network = HstsNetwork::with_new_cache(transport.clone(), clock.clone());

    network.cache().borrow_mut().observe_response(
        &url("https://expired.test/"),
        "max-age=1; includeSubDomains",
        0,
    );
    assert_eq!(network.cache().borrow().len(), 1);

    clock.set(1_000.0);
    network.start(
        1,
        FetchRequest::get(url("http://api.expired.test/resource")),
    );

    let seen = transport.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].url.to_string(),
        "http://api.expired.test/resource"
    );
    assert!(network.cache().borrow().is_empty());
}

#[test]
fn request_boundary_purges_all_expired_entries_but_keeps_live_policy() {
    let clock = Rc::new(ManualClock::new());
    let transport = Rc::new(ManualNetwork::new());
    let network = HstsNetwork::with_new_cache(transport.clone(), clock.clone());

    {
        let cache = network.cache();
        let mut cache = cache.borrow_mut();
        cache.observe_response(&url("https://expired-a.test/"), "max-age=1", 0);
        cache.observe_response(&url("https://expired-b.test/"), "max-age=1", 0);
        cache.observe_response(&url("https://live.test/"), "max-age=60", 0);
    }
    assert_eq!(network.cache().borrow().len(), 3);

    clock.set(1_500.0);
    network.start(1, FetchRequest::get(url("http://live.test/data")));

    let seen = transport.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].url.to_string(), "https://live.test/data");
    assert_eq!(network.cache().borrow().len(), 1);
    assert!(network.cache().borrow().is_known_host("live.test", 1_500));
}
