use browser_engine::{HstsCache, HstsPolicy, Url};

#[test]
fn duplicate_unknown_hsts_directive_invalidates_the_field() {
    assert_eq!(
        HstsPolicy::parse("max-age=60; preload; preload"),
        None,
        "RFC 6797 requires every directive to appear at most once"
    );

    assert_eq!(
        HstsPolicy::parse("max-age=60; Foo=1; fOO=2"),
        None,
        "directive-name matching is case-insensitive"
    );
}

#[test]
fn distinct_extension_directives_remain_ignorable() {
    assert_eq!(
        HstsPolicy::parse("max-age=60; preload; vendor-token=1"),
        Some(HstsPolicy {
            max_age_seconds: 60,
            include_subdomains: false,
        })
    );
}

#[test]
fn invalid_duplicate_extension_does_not_mutate_hsts_cache() {
    let mut cache = HstsCache::new();
    let https = Url::parse("https://example.test/").unwrap();
    let http = Url::parse("http://example.test/path").unwrap();

    assert!(cache.observe_response(&https, "max-age=120", 1_000));
    assert_eq!(
        cache.upgrade_url(&http, 2_000).to_string(),
        "https://example.test/path"
    );

    assert!(!cache.observe_response(
        &https,
        "max-age=0; preload; PRELOAD",
        3_000,
    ));
    assert!(cache.is_known_host("example.test", 3_001));
    assert_eq!(
        cache.upgrade_url(&http, 3_001).to_string(),
        "https://example.test/path"
    );
}
