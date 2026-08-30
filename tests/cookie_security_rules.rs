use browser_engine::cookie::{Cookie, CookieJar, SameSite, MAX_COOKIE_AGE_SECONDS};
use browser_engine::Url;

fn https(path: &str) -> Url {
    Url::parse(&format!("https://example.test{path}")).unwrap()
}

fn http(path: &str) -> Url {
    Url::parse(&format!("http://example.test{path}")).unwrap()
}

#[test]
fn secure_cookie_cannot_be_established_over_cleartext_http() {
    assert!(CookieJar::parse_set_cookie("sid=abc; Path=/; Secure", &http("/login"), 0,).is_none());

    let cookie = CookieJar::parse_set_cookie("sid=abc; Path=/; Secure", &https("/login"), 0)
        .expect("HTTPS may establish Secure cookies");
    assert!(cookie.secure);
}

#[test]
fn same_site_none_requires_secure_transport_and_secure_attribute() {
    assert!(
        CookieJar::parse_set_cookie("cross=1; Path=/; SameSite=None", &https("/"), 0,).is_none()
    );
    assert!(
        CookieJar::parse_set_cookie("cross=1; Path=/; SameSite=None; Secure", &http("/"), 0,)
            .is_none()
    );

    let cookie =
        CookieJar::parse_set_cookie("cross=1; Path=/; SameSite=None; Secure", &https("/"), 0)
            .expect("secure SameSite=None cookie");
    assert_eq!(cookie.same_site, SameSite::None);
    assert!(cookie.secure);
}

#[test]
fn secure_prefix_is_enforced_case_insensitively_by_the_user_agent() {
    for name in ["__Secure-id", "__secure-id", "__SECURE-id", "__SeCuRe-id"] {
        assert!(
            CookieJar::parse_set_cookie(&format!("{name}=1; Path=/"), &https("/"), 0).is_none(),
            "UA accepted prefixed cookie without Secure: {name}"
        );
        assert!(
            CookieJar::parse_set_cookie(&format!("{name}=1; Path=/; Secure"), &http("/"), 0,)
                .is_none(),
            "clear-text origin established prefixed cookie: {name}"
        );
        assert!(
            CookieJar::parse_set_cookie(&format!("{name}=1; Path=/; Secure"), &https("/"), 0,)
                .is_some(),
            "secure origin rejected valid prefixed cookie: {name}"
        );
    }
}

#[test]
fn host_prefix_requires_secure_host_only_root_path_and_explicit_path_attribute() {
    let url = https("/account/profile");

    for name in ["__Host-session", "__host-session", "__HOST-session"] {
        assert!(CookieJar::parse_set_cookie(
            &format!("{name}=1; Path=/; Secure; Domain=example.test"),
            &url,
            0,
        )
        .is_none());
        assert!(
            CookieJar::parse_set_cookie(&format!("{name}=1; Path=/account; Secure"), &url, 0,)
                .is_none()
        );
        assert!(
            CookieJar::parse_set_cookie(&format!("{name}=1; Secure"), &https("/"), 0,).is_none(),
            "Path=/ must be explicitly represented by a Path attribute"
        );
        assert!(CookieJar::parse_set_cookie(&format!("{name}=1; Path=/"), &url, 0,).is_none());

        let cookie = CookieJar::parse_set_cookie(&format!("{name}=1; Path=/; Secure"), &url, 0)
            .expect("valid __Host- family cookie");
        assert!(cookie.secure);
        assert!(cookie.host_only);
        assert_eq!(cookie.domain, "example.test");
        assert_eq!(cookie.path, "/");
    }
}

#[test]
fn nameless_cookie_cannot_smuggle_a_reserved_prefix_in_its_value() {
    for value in ["__Secure-id=1", "__secure-id=1", "__HOST-id=1"] {
        assert!(
            CookieJar::parse_set_cookie(value, &https("/"), 0).is_none(),
            "accepted nameless reserved-prefix cookie: {value}"
        );
    }
    let ordinary = CookieJar::parse_set_cookie("plain-value", &https("/"), 0)
        .expect("nameless non-prefixed cookies are permitted by UA parsing");
    assert_eq!(ordinary.name, "");
    assert_eq!(ordinary.value, "plain-value");
}

#[test]
fn ip_domain_attributes_are_exact_only() {
    let url = Url::parse("http://127.0.0.1/app").unwrap();

    assert!(CookieJar::parse_set_cookie("sid=1; Domain=0.0.1; Path=/", &url, 0,).is_none());

    let exact = CookieJar::parse_set_cookie("sid=1; Domain=127.0.0.1; Path=/", &url, 0)
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
fn trailing_dot_domain_does_not_domain_match_the_origin() {
    assert!(
        CookieJar::parse_set_cookie("sid=1; Domain=example.test.; Path=/", &https("/"), 0,)
            .is_none()
    );
}

#[test]
fn enormous_max_age_is_clamped_to_the_engine_400_day_policy() {
    let now_ms = 123;
    let cookie = CookieJar::parse_set_cookie(
        "long=1; Max-Age=9223372036854775807; Path=/",
        &https("/"),
        now_ms,
    )
    .expect("large but parseable Max-Age");

    let expected = now_ms + MAX_COOKIE_AGE_SECONDS * 1000;
    assert_eq!(cookie.expires_at_ms, Some(expected));
    assert!(!cookie.is_expired(expected - 1));
    assert!(cookie.is_expired(expected));
}
