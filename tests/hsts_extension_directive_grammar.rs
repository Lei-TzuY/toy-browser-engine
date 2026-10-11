use browser_engine::hsts::{HstsCache, HstsPolicy};
use browser_engine::net::Url;

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid test URL")
}

#[test]
fn quoted_extension_values_may_contain_semicolons() {
    assert_eq!(
        HstsPolicy::parse("max-age=60; future=\"alpha;beta\"; includeSubDomains"),
        Some(HstsPolicy {
            max_age_seconds: 60,
            include_subdomains: true,
        })
    );
}

#[test]
fn malformed_unknown_directives_invalidate_the_entire_field() {
    for header in [
        "max-age=60; bad name=value",
        "max-age=60; future=",
        "max-age=60; future=two words",
        "max-age=60; future=\"unterminated",
        "max-age=60; future=\"bad\rvalue\"",
    ] {
        assert_eq!(
            HstsPolicy::parse(header),
            None,
            "header should fail: {header:?}"
        );
    }
}

#[test]
fn malformed_extension_cannot_mutate_existing_hsts_state() {
    let source = url("https://example.test/");
    let mut cache = HstsCache::new();

    assert!(cache.observe_response(&source, "max-age=600", 0));
    assert!(cache.is_known_host("example.test", 1));

    assert!(!cache.observe_response(&source, "max-age=0; future=\"unterminated", 2,));
    assert!(cache.is_known_host("example.test", 3));
}

#[test]
fn escaped_quote_inside_extension_value_does_not_end_the_directive() {
    assert_eq!(
        HstsPolicy::parse("max-age=60; future=\"a\\\";b\"; flag"),
        Some(HstsPolicy {
            max_age_seconds: 60,
            include_subdomains: false,
        })
    );
}

#[test]
fn malformed_control_cannot_delete_policy_at_network_boundary() {
    use browser_engine::eventloop::ManualClock;
    use browser_engine::net::{FetchRequest, FetchResponse, ManualNetwork, NetworkBackend};
    use browser_engine::HstsNetwork;
    use std::rc::Rc;

    let transport = Rc::new(ManualNetwork::new());
    transport.set_auto_complete(true);
    let network = HstsNetwork::with_new_cache(transport.clone(), Rc::new(ManualClock::new()));
    assert!(network.cache().borrow_mut().observe_response(
        &url("https://example.test/"),
        "max-age=600",
        0,
    ));
    let mut response = FetchResponse::synthetic(
        url("https://example.test/update"),
        200,
        Some("text/plain"),
        Vec::new(),
    );
    response
        .headers
        .append_raw("strict-transport-security", "max-age=0;\u{000b}preload");
    transport.respond("https://example.test/update", response);
    network.start(1, FetchRequest::get(url("https://example.test/update")));
    assert_eq!(network.poll().len(), 1);
    assert!(network.cache().borrow().is_known_host("example.test", 0));
    network.start(2, FetchRequest::get(url("http://example.test/data")));
    assert_eq!(
        transport.requests().last().unwrap().url.to_string(),
        "https://example.test/data",
    );
}
