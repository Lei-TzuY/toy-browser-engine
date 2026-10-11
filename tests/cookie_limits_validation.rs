use browser_engine::cookie::{
    CookieJar, MAX_COOKIE_ATTRIBUTE_VALUE_BYTES, MAX_COOKIE_NAME_VALUE_BYTES,
    MAX_COOKIES_PER_DOMAIN, MAX_COOKIES_TOTAL,
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
fn user_agent_parser_is_deliberately_more_permissive_than_server_grammar() {
    let source = url("example.test");

    for accepted in [
        "bad name=1",
        "bad,name=1",
        "bad/name=1",
        "value=has space",
        "value=comma,here",
        r#"value=back\slash"#,
        r#"value="quoted""#,
    ] {
        assert!(
            CookieJar::parse_set_cookie(accepted, &source, 0).is_some(),
            "UA parser was stricter than RFC6265bis: {accepted:?}"
        );
    }

    let trimmed = CookieJar::parse_set_cookie("\t name \t=\t value \t; Path=/", &source, 0)
        .expect("SP/HTAB are trimmed at pair boundaries");
    assert_eq!(trimmed.name, "name");
    assert_eq!(trimmed.value, "value");
}

#[test]
fn forbidden_control_characters_are_rejected_but_htab_is_whitespace() {
    let source = url("example.test");

    for invalid in ["a=1\rX: y", "a=1\nX: y", "a=\0b", "a=\u{7f}b", "a=\u{1f}b"] {
        assert!(CookieJar::parse_set_cookie(invalid, &source, 0).is_none());
    }

    assert!(CookieJar::parse_set_cookie("\ta\t=\t1\t", &source, 0).is_some());
}

#[test]
fn size_limit_applies_to_name_plus_value_not_the_whole_set_cookie_field() {
    let source = url("example.test");

    let exact = format!("a={}", "x".repeat(MAX_COOKIE_NAME_VALUE_BYTES - 1));
    let parsed = CookieJar::parse_set_cookie(&exact, &source, 0).expect("4096 name+value bytes");
    assert_eq!(parsed.name.len() + parsed.value.len(), MAX_COOKIE_NAME_VALUE_BYTES);

    let too_large = format!("a={}", "x".repeat(MAX_COOKIE_NAME_VALUE_BYTES));
    assert!(CookieJar::parse_set_cookie(&too_large, &source, 0).is_none());

    // Oversize attributes are ignored individually; they do not make an
    // otherwise small cookie fail merely because the full field is > 4 KiB.
    let huge_attribute = format!(
        "small=1; Unknown={}; Path=/",
        "z".repeat(MAX_COOKIE_NAME_VALUE_BYTES + 500)
    );
    let parsed = CookieJar::parse_set_cookie(&huge_attribute, &source, 0)
        .expect("oversize unknown attribute is ignored");
    assert_eq!((parsed.name.as_str(), parsed.value.as_str()), ("small", "1"));
    assert_eq!(parsed.path, "/");
}

#[test]
fn oversize_known_attribute_is_ignored_and_bad_path_falls_back_to_default() {
    let source = url("example.test");
    let huge_path = format!("sid=1; Path=/{}", "x".repeat(MAX_COOKIE_ATTRIBUTE_VALUE_BYTES));
    let parsed = CookieJar::parse_set_cookie(&huge_path, &source, 0)
        .expect("cookie remains valid when one attribute is ignored");
    assert_eq!(parsed.path, "/app", "1025-byte Path value is ignored");

    let bad_path = CookieJar::parse_set_cookie("sid=1; Path=relative", &source, 0)
        .expect("invalid Path attribute falls back to default path");
    assert_eq!(bad_path.path, "/app");
}

#[test]
fn nameless_cookie_serializes_without_a_spurious_equals_sign() {
    let source = url("example.test");
    let mut jar = CookieJar::new();
    store(&mut jar, &source, "plain-value; Path=/", 0);

    assert_eq!(
        jar.get_http_cookie_header(&source, 0).as_deref(),
        Some("plain-value")
    );
}

#[test]
fn cookie_string_orders_longer_paths_before_shorter_paths() {
    let source = Url::parse("https://example.test/app/deep/page").unwrap();
    let root = Url::parse("https://example.test/").unwrap();
    let mut jar = CookieJar::new();

    store(&mut jar, &root, "root=1; Path=/", 0);
    store(&mut jar, &root, "app=1; Path=/app", 0);
    store(&mut jar, &root, "deep=1; Path=/app/deep", 0);

    assert_eq!(
        jar.get_http_cookie_header(&source, 0).as_deref(),
        Some("deep=1; app=1; root=1")
    );
}

#[test]
fn document_cookie_cannot_create_or_overwrite_httponly_state() {
    let source = url("example.test");
    let mut jar = CookieJar::new();

    jar.set_document_cookie("script=1; Path=/; HttpOnly", &source, 0);
    assert!(jar.get_http_cookie_header(&source, 0).is_none());

    let http_only = CookieJar::parse_set_cookie("sid=secret; Path=/; HttpOnly", &source, 0)
        .expect("HTTP Set-Cookie may create HttpOnly state");
    jar.store(http_only, 0);
    jar.set_document_cookie("sid=evil; Path=/", &source, 0);

    assert_eq!(
        jar.get_http_cookie_header(&source, 0).as_deref(),
        Some("sid=secret")
    );
    assert_eq!(jar.get_document_cookie(&source, 0), "");
}

#[test]
fn per_domain_limit_prefers_evicting_non_secure_state() {
    let source = url("example.test");
    let mut jar = CookieJar::new();

    store(&mut jar, &source, "secure_oldest=1; Path=/; Secure", 0);
    for i in 0..(MAX_COOKIES_PER_DOMAIN - 1) {
        store(&mut jar, &source, &format!("c{i}=v{i}; Path=/"), 0);
    }
    store(&mut jar, &source, "new=1; Path=/", 0);

    assert_eq!(jar.len(), MAX_COOKIES_PER_DOMAIN);
    let header = jar.get_http_cookie_header(&source, 0).unwrap();
    assert!(header.contains("secure_oldest=1"), "{header}");
    assert!(!header.split("; ").any(|pair| pair == "c0=v0"), "{header}");
    assert!(header.contains("new=1"), "{header}");
}

#[test]
fn replacement_preserves_creation_order_for_equal_paths() {
    let source = url("example.test");
    let mut jar = CookieJar::new();

    store(&mut jar, &source, "first=1; Path=/", 0);
    store(&mut jar, &source, "second=1; Path=/", 0);
    store(&mut jar, &source, "first=updated; Path=/", 0);

    assert_eq!(
        jar.get_http_cookie_header(&source, 0).as_deref(),
        Some("first=updated; second=1")
    );
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
    let per_domain = MAX_COOKIES_PER_DOMAIN - 1;
    let domains_needed = (MAX_COOKIES_TOTAL / per_domain) + 2;
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
