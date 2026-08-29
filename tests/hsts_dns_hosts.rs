use browser_engine::{HstsCache, Url};

fn url(input: &str) -> Url {
    Url::parse(input).unwrap()
}

#[test]
fn learned_hsts_matches_case_and_a_terminal_dns_root_dot() {
    let mut cache = HstsCache::new();
    assert!(cache.observe_response(
        &url("https://example.test/"),
        "max-age=60; includeSubDomains",
        0,
    ));

    assert!(cache.is_known_host("EXAMPLE.TEST.", 1));
    assert!(cache.is_known_host("API.EXAMPLE.TEST.", 1));
}

#[test]
fn malformed_dns_names_cannot_reach_a_parent_include_subdomains_policy() {
    let mut cache = HstsCache::new();
    assert!(cache.observe_response(
        &url("https://example.test/"),
        "max-age=60; includeSubDomains",
        0,
    ));

    for malformed in [
        "bad..api.example.test",
        "_service.api.example.test",
        "-bad.api.example.test",
        "bad-.api.example.test",
        "api.example.test..",
    ] {
        assert!(
            !cache.is_known_host(malformed, 1),
            "malformed host unexpectedly inherited HSTS: {malformed}"
        );
    }
}

#[test]
fn ascii_punycode_hosts_are_supported_but_raw_unicode_is_conservative() {
    let mut cache = HstsCache::new();
    assert!(cache.observe_response(
        &url("https://xn--bcher-kva.example/"),
        "max-age=60",
        0,
    ));

    assert!(cache.is_known_host("XN--BCHER-KVA.EXAMPLE", 1));
    assert!(!cache.is_known_host("bücher.example", 1));
}

#[test]
fn hsts_upgrade_uses_the_validated_canonical_host() {
    let mut cache = HstsCache::new();
    assert!(cache.observe_response(
        &url("https://example.test/"),
        "max-age=60; includeSubDomains",
        0,
    ));

    let upgraded = cache.upgrade_url(&url("http://api.example.test:80/path?q=1#frag"), 1);
    assert_eq!(upgraded.to_string(), "https://api.example.test:443/path?q=1#frag");

    // Public host queries that are not valid DNS names must not accidentally
    // inherit a parent's includeSubDomains policy through string suffixes.
    assert!(!cache.is_known_host("bad..api.example.test", 1));
}
