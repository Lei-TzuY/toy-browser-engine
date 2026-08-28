use browser_engine::{SecurityOrigin, Url};

fn origin(input: &str) -> SecurityOrigin {
    SecurityOrigin::of(&Url::parse(input).expect("valid URL"))
}

#[test]
fn https_is_trustworthy_independent_of_port() {
    assert!(origin("https://example.test/").is_potentially_trustworthy());
    assert!(origin("https://example.test:8443/app").is_potentially_trustworthy());
}

#[test]
fn localhost_and_loopback_http_are_trustworthy_for_development() {
    for input in [
        "http://localhost/",
        "http://localhost:8080/",
        "http://tool.dev.localhost/",
        "http://127.0.0.1/",
        "http://127.42.7.9:9000/",
        "http://[::1]/",
    ] {
        assert!(
            origin(input).is_potentially_trustworthy(),
            "{input} should be trustworthy"
        );
    }
}

#[test]
fn private_lan_and_public_cleartext_http_stay_untrusted() {
    for input in [
        "http://example.test/",
        "http://192.168.1.20/",
        "http://10.1.2.3/",
        "http://172.16.4.5/",
    ] {
        assert!(
            !origin(input).is_potentially_trustworthy(),
            "{input} must not gain secure-context trust"
        );
    }
}

#[test]
fn intentional_local_namespaces_are_trustworthy_but_opaque_urls_are_not() {
    assert!(origin("file:///tmp/index.html").is_potentially_trustworthy());
    assert!(origin("demo:///index.html").is_potentially_trustworthy());

    for input in [
        "about:blank",
        "data:text/plain,hello",
        "mailto:user@example.test",
        "urn:isbn:9780131103627",
        "widget:opaque-value",
    ] {
        assert!(
            !origin(input).is_potentially_trustworthy(),
            "{input} is opaque and must not be trusted on its own"
        );
    }
}

#[test]
fn trust_is_separate_from_same_origin_fetch_permission() {
    let source = origin("https://example.test/app/index.html");
    assert!(source.is_potentially_trustworthy());
    assert!(source.can_fetch(&Url::parse("https://example.test/api").unwrap()));
    assert!(!source.can_fetch(&Url::parse("https://other.test/api").unwrap()));

    let local = origin("demo:///site/index.html");
    assert!(local.is_potentially_trustworthy());
    assert!(local.can_fetch(&Url::parse("demo:///site/assets/app.js").unwrap()));
    assert!(!local.can_fetch(&Url::parse("demo:///other/secret.txt").unwrap()));
}
