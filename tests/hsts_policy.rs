use browser_engine::{HstsCache, HstsPolicy, Url};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

#[test]
fn public_parser_accepts_rfc_examples_and_ignores_extensions() {
    assert_eq!(
        HstsPolicy::parse("max-age=15768000 ; includeSubDomains"),
        Some(HstsPolicy {
            max_age_seconds: 15_768_000,
            include_subdomains: true,
        })
    );
    assert_eq!(
        HstsPolicy::parse("max-age=\"31536000\"; preload"),
        Some(HstsPolicy {
            max_age_seconds: 31_536_000,
            include_subdomains: false,
        })
    );
}

#[test]
fn learned_parent_policy_upgrades_subdomain_with_full_url_preservation() {
    let mut cache = HstsCache::new();
    assert!(cache.observe_response(
        &url("https://example.test/bootstrap"),
        "max-age=60; includeSubDomains",
        1_000,
    ));

    let upgraded = cache.upgrade_url(
        &url("http://assets.example.test:80/app.js?v=3#module"),
        2_000,
    );
    assert_eq!(
        upgraded.to_string(),
        "https://assets.example.test:443/app.js?v=3#module"
    );
}

#[test]
fn non_default_port_is_preserved_and_non_hsts_hosts_are_untouched() {
    let mut cache = HstsCache::new();
    cache.observe_response(&url("https://example.test/"), "max-age=60", 0);

    assert_eq!(
        cache
            .upgrade_url(&url("http://example.test:8080/api"), 1)
            .to_string(),
        "https://example.test:8080/api"
    );
    assert_eq!(
        cache
            .upgrade_url(&url("http://other.test:80/api"), 1)
            .to_string(),
        "http://other.test:80/api"
    );
}

#[test]
fn policy_expiry_and_refresh_use_absolute_monotonic_time() {
    let mut cache = HstsCache::new();
    let origin = url("https://example.test/");

    cache.observe_response(&origin, "max-age=2", 10_000);
    assert!(cache.is_known_host("example.test", 11_999));

    cache.observe_response(&origin, "max-age=5", 11_000);
    assert!(cache.is_known_host("example.test", 15_999));
    assert!(!cache.is_known_host("example.test", 16_000));
}

#[test]
fn zero_age_child_does_not_cancel_parent_include_subdomains_policy() {
    let mut cache = HstsCache::new();
    cache.observe_response(
        &url("https://example.test/"),
        "max-age=100; includeSubDomains",
        0,
    );
    cache.observe_response(
        &url("https://api.example.test/"),
        "max-age=100",
        0,
    );
    cache.observe_response(
        &url("https://api.example.test/"),
        "max-age=0",
        1,
    );

    assert!(cache.is_known_host("api.example.test", 2));
    assert_eq!(
        cache
            .upgrade_url(&url("http://api.example.test/data"), 2)
            .scheme(),
        "https"
    );
}

#[test]
fn insecure_or_ip_literal_learning_is_rejected() {
    let mut cache = HstsCache::new();
    assert!(!cache.observe_response(
        &url("http://example.test/"),
        "max-age=100; includeSubDomains",
        0,
    ));
    assert!(!cache.observe_response(
        &url("https://192.0.2.1/"),
        "max-age=100",
        0,
    ));
    assert!(!cache.is_known_host("192.0.2.1", 1));
    assert!(cache.is_empty());
}
