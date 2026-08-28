use super::*;

fn url(s: &str) -> Url {
    Url::parse(s).expect("valid url")
}

#[test]
fn parses_the_parts_of_an_http_url() {
    let u = url("http://example.com:8080/a/b.html?x=1&y=2#top");
    assert_eq!(u.scheme(), "http");
    assert_eq!(u.host(), "example.com");
    assert_eq!(u.port_or_default(), Some(8080));
    assert_eq!(u.path(), "/a/b.html");
    assert_eq!(u.query(), Some("x=1&y=2"));
    assert_eq!(u.fragment(), Some("top"));
    assert_eq!(u.request_target(), "/a/b.html?x=1&y=2");
    assert!(!u.is_opaque());
}

#[test]
fn single_letter_schemes_are_valid_absolute_urls() {
    let u = url("X:/resource?q=1#frag");
    assert_eq!(u.scheme(), "x");
    assert_eq!(u.path(), "/resource");
    assert_eq!(u.query(), Some("q=1"));
    assert_eq!(u.fragment(), Some("frag"));
    assert_eq!(u.to_string(), "x:/resource?q=1#frag");
    assert!(!u.is_opaque());
}

#[test]
fn opaque_urls_preserve_their_scheme_specific_paths() {
    for text in [
        "data:text/plain,hello",
        "data:text/plain,a/../b?x=1#frag",
        "about:blank",
        "mailto:user@example.com",
        "urn:isbn:9780131103627",
    ] {
        let parsed = url(text);
        assert!(parsed.is_opaque(), "expected opaque URL: {text}");
        assert_eq!(parsed.to_string(), text);
    }
    assert_eq!(url("data:text/plain,hello").path(), "text/plain,hello");
    assert_eq!(
        url("data:text/plain,a/../b").path(),
        "text/plain,a/../b",
        "dot segments inside opaque payloads are data, not directories"
    );
}

#[test]
fn default_ports_come_from_the_scheme() {
    assert_eq!(url("http://example.com/").port_or_default(), Some(80));
    assert_eq!(url("https://example.com/").port_or_default(), Some(443));
    assert_eq!(url("file:///tmp/x").port_or_default(), None);
}

#[test]
fn round_trips_through_display() {
    for text in [
        "http://example.com/a/b?c=d#e",
        "https://example.com/",
        "file:///tmp/page.html",
        "data:text/plain,hello?x=1#e",
        "about:blank",
    ] {
        assert_eq!(url(text).to_string(), text);
    }
}

#[test]
fn rejects_relative_references_as_absolute_urls() {
    assert!(matches!(
        Url::parse("style.css"),
        Err(UrlError::MissingScheme(_))
    ));
    assert!(matches!(
        Url::parse("/root/style.css"),
        Err(UrlError::MissingScheme(_))
    ));
    // A backslash Windows path must not be read as a `c:` scheme.
    assert!(Url::parse(r"C:\dir\page.html").is_err());
}

#[test]
fn joins_relative_paths() {
    let base = url("http://example.com/docs/guide/index.html");
    assert_eq!(
        base.join("style.css").unwrap().to_string(),
        "http://example.com/docs/guide/style.css"
    );
    assert_eq!(
        base.join("./style.css").unwrap().to_string(),
        "http://example.com/docs/guide/style.css"
    );
    assert_eq!(
        base.join("../style.css").unwrap().to_string(),
        "http://example.com/docs/style.css"
    );
    assert_eq!(
        base.join("../../style.css").unwrap().to_string(),
        "http://example.com/style.css"
    );
    assert_eq!(
        base.join("/style.css").unwrap().to_string(),
        "http://example.com/style.css"
    );
    assert_eq!(
        base.join("sub/page.html").unwrap().to_string(),
        "http://example.com/docs/guide/sub/page.html"
    );
}

#[test]
fn joins_absolute_and_protocol_relative_references() {
    let base = url("https://example.com/a/b.html");
    assert_eq!(
        base.join("http://other.test/x").unwrap().to_string(),
        "http://other.test/x"
    );
    assert_eq!(
        base.join("//cdn.test/lib.js").unwrap().to_string(),
        "https://cdn.test/lib.js"
    );
}

#[test]
fn opaque_bases_only_accept_absolute_or_component_only_references() {
    let base = url("data:text/plain,hello?old=1#old");
    assert_eq!(
        base.join("#section").unwrap().to_string(),
        "data:text/plain,hello?old=1#section"
    );
    assert_eq!(
        base.join("?new=2").unwrap().to_string(),
        "data:text/plain,hello?new=2"
    );
    assert_eq!(
        base.join("https://example.com/x").unwrap().to_string(),
        "https://example.com/x"
    );
    assert!(base.join("child").is_err());
    assert!(base.join("//cdn.example/x").is_err());
}

#[test]
fn joins_query_and_fragment_only_references() {
    let base = url("http://example.com/a/b.html?old=1#old");
    assert_eq!(
        base.join("#section").unwrap().to_string(),
        "http://example.com/a/b.html?old=1#section"
    );
    assert_eq!(
        base.join("?new=2").unwrap().to_string(),
        "http://example.com/a/b.html?new=2"
    );
    assert_eq!(
        base.join("").unwrap().to_string(),
        "http://example.com/a/b.html?old=1"
    );
}

#[test]
fn dot_segments_cannot_escape_the_root() {
    let base = url("http://example.com/a.html");
    assert_eq!(
        base.join("../../../x.css").unwrap().to_string(),
        "http://example.com/x.css"
    );
}

#[test]
fn directory_bases_keep_their_trailing_slash() {
    let base = url("http://example.com/docs/");
    assert_eq!(
        base.join("a.html").unwrap().to_string(),
        "http://example.com/docs/a.html"
    );
    assert_eq!(
        base.join("../a.html").unwrap().to_string(),
        "http://example.com/a.html"
    );
}

#[test]
fn file_urls_round_trip_through_paths() {
    let original = std::env::current_dir()
        .unwrap()
        .join("sub")
        .join("page.html");
    let u = Url::from_file_path(&original);
    assert_eq!(u.scheme(), "file");
    assert_eq!(u.to_file_path().unwrap(), original);
}

#[test]
fn file_urls_resolve_relative_references() {
    let base = Url::from_file_path("/site/docs/index.html");
    let joined = base.join("../assets/logo.png").unwrap();
    assert_eq!(joined.scheme(), "file");
    assert!(
        joined.path().ends_with("/site/assets/logo.png"),
        "got {joined}"
    );
}

#[test]
fn spaces_in_paths_are_encoded_and_decoded() {
    // Built from a real absolute path so the test holds on every platform.
    let path = std::env::temp_dir().join("my dir").join("a b.html");
    let u = Url::from_file_path(&path);
    assert!(u.to_string().contains("%20"), "got {u}");
    assert_eq!(u.to_file_path().unwrap(), path);
}

#[test]
fn dropping_the_query_gives_the_file_identity() {
    let u = url("http://example.com/search?q=1#top");
    assert_eq!(
        u.without_query_and_fragment().to_string(),
        "http://example.com/search"
    );
}

#[test]
fn same_document_ignores_the_fragment() {
    let a = url("http://example.com/x.html#one");
    let b = url("http://example.com/x.html#two");
    let c = url("http://example.com/y.html");
    assert!(a.same_document(&b));
    assert!(!a.same_document(&c));
}

#[test]
fn credentials_and_ipv6_hosts_parse() {
    assert_eq!(url("http://user:pw@example.com/x").host(), "example.com");
    assert_eq!(url("http://[::1]:9000/x").host(), "[::1]");
    assert_eq!(url("http://[::1]:9000/x").port_or_default(), Some(9000));
}

#[test]
fn hosts_and_schemes_are_lowercased() {
    let u = url("HTTP://Example.COM/Path");
    assert_eq!(u.scheme(), "http");
    assert_eq!(u.host(), "example.com");
    // Paths stay case-sensitive.
    assert_eq!(u.path(), "/Path");
}
