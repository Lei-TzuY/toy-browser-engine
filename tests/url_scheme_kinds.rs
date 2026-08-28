use browser_engine::Url;

#[test]
fn engine_resource_schemes_remain_hierarchical_without_double_slashes() {
    for input in ["http:foo/bar", "https:foo/bar", "file:tmp/a", "demo:pages/a"] {
        let url = Url::parse(input).unwrap();
        assert!(!url.is_opaque(), "{input} must stay hierarchical");
        assert!(url.path().starts_with('/'), "normalized path for {input}: {}", url.path());
    }

    let http = Url::parse("http:docs/guide/index.html").unwrap();
    assert_eq!(http.to_string(), "http:/docs/guide/index.html");
    assert_eq!(
        http.join("../asset.png").unwrap().to_string(),
        "http:/docs/asset.png"
    );
}

#[test]
fn intrinsically_opaque_schemes_ignore_slash_shape() {
    for input in [
        "data:text/plain,hello",
        "data:/text/plain,hello",
        "data://text/plain,hello",
        "about:blank",
        "about:/blank",
        "mailto:user@example.com",
        "mailto:/user@example.com",
        "urn:isbn:9780131103627",
        "urn:/isbn:9780131103627",
    ] {
        let url = Url::parse(input).unwrap();
        assert!(url.is_opaque(), "{input} must be opaque");
        assert!(url.host().is_empty(), "opaque URL acquired a host: {input}");
        assert_eq!(url.to_string(), input);
    }
}

#[test]
fn unknown_schemes_fall_back_to_shape_based_classification() {
    let opaque = Url::parse("widget:asset").unwrap();
    assert!(opaque.is_opaque());
    assert_eq!(opaque.to_string(), "widget:asset");
    assert!(opaque.join("child").is_err());

    let rooted = Url::parse("widget:/a/../asset").unwrap();
    assert!(!rooted.is_opaque());
    assert_eq!(rooted.to_string(), "widget:/asset");

    let authority = Url::parse("widget://Example.COM/a/../asset").unwrap();
    assert!(!authority.is_opaque());
    assert_eq!(authority.host(), "example.com");
    assert_eq!(authority.to_string(), "widget://example.com/asset");
}

#[test]
fn known_opaque_urls_never_gain_authority_from_network_path_text() {
    let data = Url::parse("data://host/path#frag").unwrap();
    assert!(data.is_opaque());
    assert_eq!(data.host(), "");
    assert_eq!(data.path(), "//host/path");
    assert_eq!(data.to_string(), "data://host/path#frag");

    // Network-path text is not a relative reference for cannot-be-a-base URLs.
    assert!(Url::parse("about:blank").unwrap().join("//example.com/x").is_err());
}
