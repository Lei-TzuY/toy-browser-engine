use browser_engine::cookie::{
    CookieJar, MAX_COOKIES_PER_DOMAIN, MAX_COOKIES_TOTAL, MAX_SET_COOKIE_BYTES,
};
use browser_engine::Url;

fn url(host: &str) -> Url {
    Url::parse(&format!("https://{host}/app/index.html")).unwrap()
}

fn store(jar: &mut CookieJar, source: &Url, text: &str, now_ms: u64) {
    let cookie = CookieJar::parse_set_cookie(text, source, now_ms)
        .unwrap_or_else(|| panic!("valid test cookie rejected: {text}"));
    jar.store(cookie, now_ms);
}

#[test]
fn cookie_names_must_be_http_tokens() {
    let source = url("example.test");

    for invalid in [
        "bad name=1",
        "bad,name=1",
        "bad/name=1",
        "bad(name=1",
        "bad@name=1",
        "bad:name=1",
        "bad[name=1",
    ] {
        assert!(
            CookieJar::parse_set_cookie(invalid, &source, 0).is_none(),
            "accepted invalid name: {invalid:?}"
        );
    }

    for valid in [
        "a=1",
        "A-Z_09=ok",
        "!#$%&'*+-.^_`|~=token",
    ] {
        assert!(
            CookieJar::parse_set_cookie(valid, &source, 0).is_some(),
            "rejected token name: {valid:?}"
        );
    }
}

#[test]
fn cookie_values_follow_cookie_octet_rules_and_support_quotes() {
    let source = url("example.test");

    let quoted = CookieJar::parse_set_cookie(r#"theme="dark-mode"; Path=/"#, &source, 0)
        .expect("quoted cookie-octet value");
    assert_eq!(quoted.value, r#""dark-mode""#);

    for invalid in [
        "value=has space",
        "value=comma,here",
        r#"value=back\slash"#,
        r#"value="unterminated"#,
        r#"value=unterminated""#,
    ] {
        assert!(
            CookieJar::parse_set_cookie(invalid, &source, 0).is_none(),
            "accepted invalid cookie value: {invalid:?}"
        );
    }
}

#[test]
fn control_characters_and_oversize_fields_are_rejected() {
    let source = url("example.test");

    for invalid in ["a=1\rX: y", "a=1\nX: y", "a=\0b", "a=\u{7f}b"] {
        assert!(CookieJar::parse_set_cookie(invalid, &source, 0).is_none());
    }

    let exact = format!("a={}", "x".repeat(MAX_SET_COOKIE_BYTES - 2));
    assert_eq!(exact.len(), MAX_SET_COOKIE_BYTES);
    assert!(CookieJar::parse_set_cookie(&exact, &source, 0).is_some());

    let too_large = format!("a={}", "x".repeat(MAX_SET_COOKIE_BYTES - 1));
    assert_eq!(too_large.len(), MAX_SET_COOKIE_BYTES + 1);
    assert!(CookieJar::parse_set_cookie(&too_large, &source, 0).is_none());
}

#[test]
fn per_domain_limit_evicts_oldest_cookie() {
    let source = url("example.test");
    let mut jar = CookieJar::new();

    for i in 0..=MAX_COOKIES_PER_DOMAIN {
        store(&mut jar, &source, &format!("c{i}=v{i}; Path=/"), 0);
    }

    assert_eq!(jar.len(), MAX_COOKIES_PER_DOMAIN);
    let header = jar.get_http_cookie_header(&source, 0).unwrap();
    assert!(!header.split("; ").any(|pair| pair == "c0=v0"), "{header}");
    assert!(
        header
            .split("; ")
            .any(|pair| pair == format!("c{}=v{}", MAX_COOKIES_PER_DOMAIN, MAX_COOKIES_PER_DOMAIN)),
        "{header}"
    );
}

#[test]
fn updating_a_cookie_refreshes_its_eviction_recency() {
    let source = url("example.test");
    let mut jar = CookieJar::new();

    for i in 0..MAX_COOKIES_PER_DOMAIN {
        store(&mut jar, &source, &format!("c{i}=v{i}; Path=/"), 0);
    }

    store(&mut jar, &source, "c0=refreshed; Path=/", 0);
    store(
        &mut jar,
        &source,
        &format!("c{}=new; Path=/", MAX_COOKIES_PER_DOMAIN),
        0,
    );

    let header = jar.get_http_cookie_header(&source, 0).unwrap();
    assert!(header.split("; ").any(|pair| pair == "c0=refreshed"), "{header}");
    assert!(!header.split("; ").any(|pair| pair == "c1=v1"), "{header}");
}

#[test]
fn expired_entries_are_purged_before_capacity_eviction() {
    let source = url("example.test");
    let mut jar = CookieJar::new();

    store(&mut jar, &source, "short=1; Max-Age=1; Path=/", 0);
    for i in 0..(MAX_COOKIES_PER_DOMAIN - 1) {
        store(&mut jar, &source, &format!("live{i}=1; Path=/"), 0);
    }
    assert_eq!(jar.len(), MAX_COOKIES_PER_DOMAIN);

    // At t=1000 the short cookie is expired. Inserting one live cookie should
    // purge it first, not evict the oldest still-live cookie.
    store(&mut jar, &source, "new=1; Path=/", 1000);
    assert_eq!(jar.len(), MAX_COOKIES_PER_DOMAIN);
    let header = jar.get_http_cookie_header(&source, 1000).unwrap();
    assert!(!header.contains("short=1"));
    assert!(header.contains("live0=1"), "{header}");
    assert!(header.contains("new=1"), "{header}");
}

#[test]
fn total_session_limit_bounds_cross_domain_growth() {
    let mut jar = CookieJar::new();
    let domains_needed = (MAX_COOKIES_TOTAL / (MAX_COOKIES_PER_DOMAIN - 1)) + 2;
    let per_domain = MAX_COOKIES_PER_DOMAIN - 1;
    let mut first_source = None;

    for domain_index in 0..domains_needed {
        let source = url(&format!("d{domain_index}.example.test"));
        if first_source.is_none() {
            first_source = Some(source.clone());
        }
        for cookie_index in 0..per_domain {
            store(
                &mut jar,
                &source,
                &format!("c{cookie_index}=d{domain_index}; Path=/"),
                0,
            );
        }
    }

    assert_eq!(jar.len(), MAX_COOKIES_TOTAL);
    let first_source = first_source.unwrap();
    let first_header = jar.get_http_cookie_header(&first_source, 0);
    assert!(
        first_header
            .as_deref()
            .is_none_or(|header| !header.split("; ").any(|pair| pair == "c0=d0")),
        "globally oldest cookie should have been evicted: {first_header:?}"
    );

    let newest_domain = url(&format!("d{}.example.test", domains_needed - 1));
    let newest_header = jar.get_http_cookie_header(&newest_domain, 0).unwrap();
    assert!(
        newest_header.contains(&format!("c{}=d{}", per_domain - 1, domains_needed - 1)),
        "{newest_header}"
    );
}
