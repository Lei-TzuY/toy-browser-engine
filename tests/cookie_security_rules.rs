use browser_engine::cookie::{Cookie, CookieJar, SameSite};
use browser_engine::Url;

fn https(path: &str) -> Url {
    Url::parse(&format!("https://example.test{path}")).unwrap()
}

fn http(path: &str) -> Url {
    Url::parse(&format!("http://example.test{path}")).unwrap()
}

#[test]
fn secure_cookie_cannot_be_established_over_cleartext_http() {
    assert!(CookieJar::parse_set_cookie(
        "sid=abc; Path=/; Secure",
        &http("/login"),
        0,
    )
    .is_none());

    let cookie = CookieJar::parse_set_cookie(
        "sid=abc; Path=/; Secure",
        &https("/login"),
        0,
    )
    .expect("HTTPS may establish Secure cookies");
    assert!(cookie.secure);
}

#[test]
fn same_site_none_requires_secure_transport_and_secure_attribute() {
    assert!(CookieJar::parse_set_cookie(
        "cross=1; Path=/; SameSite=None",
        &https("/"),
        0,
    )
    .is_none());
    assert!(CookieJar::parse_set_cookie(
        "cross=1; Path=/; SameSite=None; Secure",
        &http("/"),
        0,
    )
    .is_none());

    let cookie = CookieJar::parse_set_cookie(
        "cross=1; Path=/; SameSite=None; Secure",
        &https("/"),
        0,
    )
    .expect("secure SameSite=None cookie");
    assert_eq!(cookie.same_site, SameSite::None);
    assert!(cookie.secure);
}

#[test]
fn secure_prefix_requires_secure_https_cookie() {
    assert!(CookieJar::parse_set_cookie(
        "__Secure-id=1; Path=/",
        &https("/"),
        0,
    )
    .is_none());
    assert!(CookieJar::parse_set_cookie(
        "__Secure-id=1; Path=/; Secure",
        &http("/"),
        0,
    )
    .is_none());

    let cookie = CookieJar::parse_set_cookie(
        "__Secure-id=1; Path=/; Secure",
        &https("/"),
        0,
    )
    .expect("valid __Secure- cookie");
    assert!(cookie.secure);
}

#[test]
fn host_prefix_requires_secure_host_only_root_path() {
    let url = https("/account/profile");

    assert!(CookieJar::parse_set_cookie(
        "__Host-session=1; Path=/; Secure; Domain=example.test",
        &url,
        0,
    )
    .is_none());
    assert!(CookieJar::parse_set_cookie(
        "__Host-session=1; Path=/account; Secure",
        &url,
        0,
    )
    .is_none());
    assert!(CookieJar::parse_set_cookie(
        "__Host-session=1; Path=/",
        &url,
        0,
    )
    .is_none());

    let cookie = CookieJar::parse_set_cookie(
        "__Host-session=1; Path=/; Secure",
        &url,
        0,
    )
    .expect("valid __Host- cookie");
    assert!(cookie.secure);
    assert!(cookie.host_only);
    assert_eq!(cookie.domain, "example.test");
    assert_eq!(cookie.path, "/");
}

#[test]
fn cookie_prefix_matching_is_case_sensitive() {
    let cookie = CookieJar::parse_set_cookie(
        "__secure-id=1; Path=/",
        &https("/"),
        0,
    )
    .expect("lower-case spelling is not the reserved prefix");
    assert!(!cookie.secure);
}

#[test]
fn ip_domain_attributes_are_exact_only() {
    let url = Url::parse("http://127.0.0.1/app").unwrap();

    assert!(CookieJar::parse_set_cookie(
        "sid=1; Domain=0.0.1; Path=/",
        &url,
        0,
    )
    .is_none());

    let exact = CookieJar::parse_set_cookie(
        "sid=1; Domain=127.0.0.1; Path=/",
        &url,
        0,
    )
    .expect("an exact IP Domain is not broadened into a suffix");
    assert!(exact.matches_domain("127.0.0.1"));
    assert!(!exact.matches_domain("127.0.0.2"));

    let manually_constructed = Cookie {
        name: "manual".into(),
        value: "1".into(),
        domain: "0.0.1".into(),
        host_only: false,
        path: "/".into(),
        expires_at_ms: None,
        secure: false,
        http_only: false,
        same_site: SameSite::Lax,
    };
    assert!(
        !manually_constructed.matches_domain("127.0.0.1"),
        "manual cookies must not regain IP suffix matching"
    );
}

#[test]
fn trailing_dot_domain_is_not_normalized_into_a_valid_cookie() {
    assert!(CookieJar::parse_set_cookie(
        "sid=1; Domain=example.test.; Path=/",
        &https("/"),
        0,
    )
    .is_none());
}

#[test]
fn enormous_max_age_saturates_instead_of_overflowing() {
    let cookie = CookieJar::parse_set_cookie(
        "long=1; Max-Age=9223372036854775807; Path=/",
        &https("/"),
        123,
    )
    .expect("large but parseable Max-Age");

    assert_eq!(cookie.expires_at_ms, Some(u64::MAX));
    assert!(!cookie.is_expired(u64::MAX - 1));
    assert!(cookie.is_expired(u64::MAX));
}
