use browser_engine::{SecurityOrigin, Url};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

#[test]
fn tuple_origin_normalizes_default_ports_and_blocks_cross_origin_targets() {
    let origin = SecurityOrigin::of(&url("https://Example.Test/account"));

    assert_eq!(origin.header_value(), "https://example.test");
    assert!(origin.can_fetch(&url("https://example.test:443/api")));
    assert!(!origin.can_fetch(&url("https://example.test:444/api")));
    assert!(!origin.can_fetch(&url("http://example.test/api")));
}

#[test]
fn opaque_urls_do_not_become_local_origins_just_because_they_lack_a_host() {
    for input in [
        "about:blank",
        "data:text/plain,hello",
        "mailto:user@example.test",
        "urn:uuid:12345678-1234-1234-1234-123456789abc",
        "widget:asset",
        "http:hostless",
    ] {
        let parsed = url(input);
        let origin = SecurityOrigin::of(&parsed);
        assert!(origin.is_opaque(), "{input} should classify as opaque");
        assert!(!origin.can_fetch(&parsed), "opaque origins are unique");
        assert_eq!(origin.header_value(), "null");
    }
}

#[test]
fn demo_and_file_origins_keep_directory_subtree_confinement() {
    let demo = SecurityOrigin::of(&url("demo:///site/pages/index.html"));
    assert!(demo.can_fetch(&url("demo:///site/pages/assets/app.js")));
    assert!(!demo.can_fetch(&url("demo:///site/secrets.txt")));
    assert!(!demo.can_fetch(&url("file:///site/pages/assets/app.js")));

    let file = SecurityOrigin::of(&url("file:///home/user/site/index.html"));
    assert!(file.can_fetch(&url("file:///home/user/site/assets/app.css")));
    assert!(!file.can_fetch(&url("file:///home/user/private.txt")));
    assert!(!file.can_fetch(&url("https://example.test/assets/app.css")));
}
