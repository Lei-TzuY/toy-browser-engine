use browser_engine::{ReferrerPolicy, Url};

fn url(text: &str) -> Url {
    Url::parse(text).unwrap()
}

#[test]
fn policy_tokens_round_trip_and_header_fallback_uses_last_supported_value() {
    let policies = [
        ReferrerPolicy::NoReferrer,
        ReferrerPolicy::NoReferrerWhenDowngrade,
        ReferrerPolicy::Origin,
        ReferrerPolicy::OriginWhenCrossOrigin,
        ReferrerPolicy::SameOrigin,
        ReferrerPolicy::StrictOrigin,
        ReferrerPolicy::StrictOriginWhenCrossOrigin,
        ReferrerPolicy::UnsafeUrl,
    ];

    for policy in policies {
        assert_eq!(ReferrerPolicy::parse_token(policy.as_str()), Some(policy));
    }
    assert_eq!(
        ReferrerPolicy::from_header("no-referrer, unsupported-v2, strict-origin"),
        Some(ReferrerPolicy::StrictOrigin)
    );
}

#[test]
fn default_policy_sends_full_same_origin_origin_cross_origin_and_nothing_on_downgrade() {
    let source = url("https://www.example.test/docs/page.html?lang=en#private");
    let policy = ReferrerPolicy::default();

    assert_eq!(
        policy.compute(&source, &url("https://www.example.test/assets/app.js")),
        Some("https://www.example.test/docs/page.html?lang=en".into())
    );
    assert_eq!(
        policy.compute(&source, &url("https://cdn.example.test/app.js")),
        Some("https://www.example.test/".into())
    );
    assert_eq!(
        policy.compute(&source, &url("http://www.example.test/app.js")),
        None
    );
}

#[test]
fn origin_policy_normalizes_default_ports_and_retains_nondefault_ports() {
    assert_eq!(
        ReferrerPolicy::Origin.compute(
            &url("https://example.test:443/a"),
            &url("https://other.test/b")
        ),
        Some("https://example.test/".into())
    );
    assert_eq!(
        ReferrerPolicy::Origin.compute(
            &url("https://example.test:8443/a"),
            &url("https://other.test/b")
        ),
        Some("https://example.test:8443/".into())
    );
}

#[test]
fn no_referrer_when_downgrade_and_unsafe_url_have_distinct_https_to_http_behavior() {
    let source = url("https://secure.example.test/account?id=7#secret");
    let target = url("http://legacy.example.test/");

    assert_eq!(
        ReferrerPolicy::NoReferrerWhenDowngrade.compute(&source, &target),
        None
    );
    assert_eq!(
        ReferrerPolicy::UnsafeUrl.compute(&source, &target),
        Some("https://secure.example.test/account?id=7".into())
    );
}

#[test]
fn same_origin_compares_effective_ports() {
    let source = url("http://example.test:80/a");
    assert!(ReferrerPolicy::SameOrigin
        .compute(&source, &url("http://example.test/b"))
        .is_some());
    assert_eq!(
        ReferrerPolicy::SameOrigin.compute(&source, &url("http://example.test:8080/b")),
        None
    );
}

#[test]
fn referrer_longer_than_4096_characters_is_reduced_to_origin() {
    let prefix = "https://example.test/";
    let source = url(&format!("{prefix}{}", "a".repeat(5000)));
    let target = url("https://other.test/resource");

    assert!(source.without_fragment().to_string().chars().count() > 4096);
    assert_eq!(
        ReferrerPolicy::UnsafeUrl.compute(&source, &target),
        Some("https://example.test/".into())
    );
    assert_eq!(
        ReferrerPolicy::SameOrigin.compute(&source, &url("https://example.test/next")),
        Some("https://example.test/".into())
    );
}

#[test]
fn non_http_sources_and_targets_never_produce_a_referer_header() {
    assert_eq!(
        ReferrerPolicy::UnsafeUrl.compute(
            &url("file:///tmp/index.html"),
            &url("https://example.test/")
        ),
        None
    );
    assert_eq!(
        ReferrerPolicy::UnsafeUrl.compute(
            &url("https://example.test/index.html"),
            &url("file:///tmp/asset")
        ),
        None
    );
}
