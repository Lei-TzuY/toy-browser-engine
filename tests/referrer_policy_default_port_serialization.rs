use browser_engine::net::Url;
use browser_engine::referrer_policy::ReferrerPolicy;

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

#[test]
fn explicit_https_default_port_is_omitted_from_full_referrer() {
    let source = url("https://page.test:443/private/path?q=1#secret");
    let target = url("https://page.test/next");

    assert_eq!(
        ReferrerPolicy::SameOrigin.compute(&source, &target),
        Some("https://page.test/private/path?q=1".to_string())
    );
}

#[test]
fn explicit_http_default_port_is_omitted_from_origin_referrer() {
    let source = url("http://page.test:80/private/path?q=1");
    let target = url("http://other.test/resource");

    assert_eq!(
        ReferrerPolicy::Origin.compute(&source, &target),
        Some("http://page.test/".to_string())
    );
}

#[test]
fn nondefault_port_is_preserved() {
    let source = url("https://page.test:8443/private/path?q=1#secret");
    let target = url("https://other.test/resource");

    assert_eq!(
        ReferrerPolicy::UnsafeUrl.compute(&source, &target),
        Some("https://page.test:8443/private/path?q=1".to_string())
    );
    assert_eq!(
        ReferrerPolicy::Origin.compute(&source, &target),
        Some("https://page.test:8443/".to_string())
    );
}

#[test]
fn ipv6_authority_remains_bracketed_while_default_port_is_removed() {
    let source = url("https://[2001:db8::1]:443/private?q=1");
    let target = url("https://other.test/resource");

    assert_eq!(
        ReferrerPolicy::UnsafeUrl.compute(&source, &target),
        Some("https://[2001:db8::1]/private?q=1".to_string())
    );
}
