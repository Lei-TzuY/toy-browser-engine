use browser_engine::net::{DefaultLoader, ResourceLoader, Url};

#[test]
fn data_urls_round_trip_without_a_synthetic_slash() {
    let url = Url::parse("data:text/plain,a/../b?yes#frag").unwrap();
    assert!(url.is_opaque());
    assert_eq!(url.path(), "text/plain,a/../b");
    assert_eq!(url.query(), Some("yes"));
    assert_eq!(url.fragment(), Some("frag"));
    assert_eq!(url.to_string(), "data:text/plain,a/../b?yes#frag");
}

#[test]
fn data_loader_preserves_opaque_payload_and_question_mark_content() {
    let url = Url::parse("data:text/plain,a/../b?yes#ignored").unwrap();
    let resource = DefaultLoader::new().load(&url).unwrap();
    assert_eq!(resource.effective_mime(), "text/plain");
    assert_eq!(resource.bytes, b"a/../b?yes");
    assert_eq!(resource.url.to_string(), url.to_string());
}

#[test]
fn opaque_urls_are_not_directory_bases() {
    let data = Url::parse("data:text/plain,hello").unwrap();
    let about = Url::parse("about:blank").unwrap();

    assert!(data.join("child.png").is_err());
    assert!(data.join("//cdn.example/cursor.png").is_err());
    assert!(about.join("settings").is_err());

    assert_eq!(
        data.join("#section").unwrap().to_string(),
        "data:text/plain,hello#section"
    );
    assert_eq!(
        data.join("https://example.com/x").unwrap().to_string(),
        "https://example.com/x"
    );
}

#[test]
fn hierarchical_urls_keep_existing_normalization() {
    let base = Url::parse("https://example.com/a/b/index.html").unwrap();
    assert!(!base.is_opaque());
    assert_eq!(
        base.join(".././asset.png").unwrap().to_string(),
        "https://example.com/a/asset.png"
    );

    let one_letter = Url::parse("x:/a/../resource?q=1#frag").unwrap();
    assert!(!one_letter.is_opaque());
    assert_eq!(one_letter.to_string(), "x:/resource?q=1#frag");
}
