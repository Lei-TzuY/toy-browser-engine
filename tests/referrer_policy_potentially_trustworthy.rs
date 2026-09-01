use browser_engine::net::Url;
use browser_engine::referrer_policy::ReferrerPolicy;

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

#[test]
fn https_to_localhost_is_not_a_strict_referrer_downgrade() {
    let source = url("https://secure.test/private/path?q=1#secret");

    for target in [
        "http://localhost:8080/resource",
        "http://dev.localhost/resource",
        "http://127.0.0.42/resource",
        "http://[::1]/resource",
    ] {
        assert_eq!(
            ReferrerPolicy::StrictOriginWhenCrossOrigin.compute(&source, &url(target)),
            Some("https://secure.test/".to_string()),
            "unexpected strict-policy result for {target}"
        );
    }
}

#[test]
fn trustworthy_http_source_to_public_http_is_a_downgrade() {
    for source in [
        "http://localhost/private?q=1",
        "http://app.localhost/private?q=1",
        "http://127.0.0.1/private?q=1",
        "http://[::1]/private?q=1",
    ] {
        assert_eq!(
            ReferrerPolicy::NoReferrerWhenDowngrade
                .compute(&url(source), &url("http://public.test/resource")),
            None,
            "trustworthy source must suppress referrer toward untrustworthy HTTP: {source}"
        );
    }
}

#[test]
fn ordinary_http_to_http_remains_non_downgrade() {
    let source = url("http://page.test/private?q=1#secret");
    let target = url("http://other.test/resource");

    assert_eq!(
        ReferrerPolicy::NoReferrerWhenDowngrade.compute(&source, &target),
        Some("http://page.test/private?q=1".to_string())
    );
}

#[test]
fn non_loopback_private_addresses_are_not_implicitly_trustworthy() {
    let source = url("https://secure.test/private");

    for target in [
        "http://192.168.1.10/resource",
        "http://10.0.0.8/resource",
        "http://172.16.0.4/resource",
    ] {
        assert_eq!(
            ReferrerPolicy::StrictOriginWhenCrossOrigin.compute(&source, &url(target)),
            None,
            "private network address is not automatically potentially trustworthy: {target}"
        );
    }
}
