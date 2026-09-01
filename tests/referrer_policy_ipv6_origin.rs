use browser_engine::net::Url;
use browser_engine::referrer_policy::ReferrerPolicy;

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

#[test]
fn cross_origin_ipv6_referrer_uses_bracketed_origin() {
    let source = url("https://[2001:db8::1]/private/path?q=1#secret");
    let target = url("https://example.test/resource");

    assert_eq!(
        ReferrerPolicy::StrictOriginWhenCrossOrigin.compute(&source, &target),
        Some("https://[2001:db8::1]/".to_string())
    );
}

#[test]
fn ipv6_origin_preserves_nondefault_port() {
    let source = url("https://[2001:db8::1]:8443/private/path");
    let target = url("https://example.test/resource");

    assert_eq!(
        ReferrerPolicy::Origin.compute(&source, &target),
        Some("https://[2001:db8::1]:8443/".to_string())
    );
}

#[test]
fn same_origin_ipv6_keeps_full_referrer_serialization() {
    let source = url("https://[2001:db8::1]/private/path?q=1#secret");
    let target = url("https://[2001:db8::1]/next");

    assert_eq!(
        ReferrerPolicy::StrictOriginWhenCrossOrigin.compute(&source, &target),
        Some("https://[2001:db8::1]/private/path?q=1".to_string())
    );
}
