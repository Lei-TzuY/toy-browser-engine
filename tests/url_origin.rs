use browser_engine::Url;

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

#[test]
fn serializes_http_and_https_tuple_origins() {
    assert_eq!(
        url("http://Example.COM/docs/page.html?x=1#top").origin().as_deref(),
        Some("http://example.com")
    );
    assert_eq!(
        url("https://example.com:8443/path").origin().as_deref(),
        Some("https://example.com:8443")
    );
}

#[test]
fn default_ports_are_normalized_in_origin_serialization() {
    assert_eq!(
        url("http://example.com:80/").origin().as_deref(),
        Some("http://example.com")
    );
    assert_eq!(
        url("https://example.com:443/").origin().as_deref(),
        Some("https://example.com")
    );
}

#[test]
fn same_origin_uses_scheme_host_and_effective_port_only() {
    let base = url("https://example.com/a/index.html?x=1#one");
    assert!(base.same_origin(&url("https://EXAMPLE.com/b/other.html?y=2#two")));
    assert!(base.same_origin(&url("https://example.com:443/explicit-default")));

    assert!(!base.same_origin(&url("http://example.com/a")));
    assert!(!base.same_origin(&url("https://other.example/a")));
    assert!(!base.same_origin(&url("https://example.com:444/a")));
}

#[test]
fn opaque_and_local_urls_do_not_gain_tuple_origins() {
    for input in [
        "data:text/plain,hello",
        "about:blank",
        "mailto:user@example.com",
        "urn:isbn:9780131103627",
        "file:///tmp/page.html",
        "demo:///page.html",
    ] {
        let parsed = url(input);
        assert_eq!(parsed.origin(), None, "unexpected tuple origin for {input}");
        assert!(
            !parsed.same_origin(&parsed),
            "non-tuple URL must not become same-origin by string equality: {input}"
        );
    }
}

#[test]
fn hostless_http_urls_do_not_claim_an_origin() {
    let parsed = url("http:/path-only");
    assert_eq!(parsed.origin(), None);
    assert!(!parsed.same_origin(&parsed));
}
