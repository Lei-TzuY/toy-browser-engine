use browser_engine::net::Url;

#[test]
fn public_url_parser_accepts_single_letter_schemes() {
    let url = Url::parse("X:/resource?q=1#frag").expect("single-letter scheme");

    assert_eq!(url.scheme(), "x");
    assert_eq!(url.path(), "/resource");
    assert_eq!(url.query(), Some("q=1"));
    assert_eq!(url.fragment(), Some("frag"));
    assert_eq!(url.to_string(), "x:/resource?q=1#frag");
}

#[test]
fn join_treats_single_letter_scheme_reference_as_absolute() {
    let base = Url::parse("demo:///dir/page.html").unwrap();
    let joined = base
        .join("x:/other?from=join#target")
        .expect("absolute single-letter reference");

    assert_eq!(joined.scheme(), "x");
    assert_eq!(joined.path(), "/other");
    assert_eq!(joined.query(), Some("from=join"));
    assert_eq!(joined.fragment(), Some("target"));
    assert_eq!(joined.to_string(), "x:/other?from=join#target");
}

#[test]
fn backslash_windows_drive_path_is_still_not_claimed_as_a_uri() {
    assert!(Url::parse(r"C:\dir\page.html").is_err());
}
