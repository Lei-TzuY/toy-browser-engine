use std::cell::RefCell;
use std::rc::Rc;

use browser_engine::cookie::CookieJar;
use browser_engine::cookie_network::CookieNetwork;
use browser_engine::eventloop::ManualClock;
use browser_engine::net::{FetchRequest, FetchResponse, ManualNetwork, NetworkBackend, Url};

fn url(value: &str) -> Url {
    Url::parse(value).unwrap()
}

fn seed_secure(jar: &mut CookieJar, path: &str) {
    let source = url("https://example.test/login");
    assert!(jar.store_set_cookie(
        &format!("sid=good; Path={path}; Secure; HttpOnly"),
        &source,
        0,
    ));
}

#[test]
fn insecure_http_cannot_replace_or_delete_overlapping_secure_cookie() {
    let mut jar = CookieJar::new();
    seed_secure(&mut jar, "/login");
    let insecure = url("http://example.test/login");

    assert!(!jar.store_set_cookie("sid=evil; Path=/login", &insecure, 0));
    assert!(!jar.store_set_cookie(
        "sid=; Path=/login; Max-Age=0",
        &insecure,
        0,
    ));

    assert_eq!(
        jar.get_http_cookie_header(&url("https://example.test/login"), 0)
            .as_deref(),
        Some("sid=good")
    );
}

#[test]
fn secure_integrity_path_comparison_is_deliberately_non_symmetric() {
    let mut jar = CookieJar::new();
    seed_secure(&mut jar, "/login");

    assert!(!jar.store_set_cookie(
        "sid=deeper; Path=/login/en",
        &url("http://example.test/login/en"),
        0,
    ));

    assert!(jar.store_set_cookie(
        "sid=broad; Path=/",
        &url("http://example.test/"),
        0,
    ));

    assert_eq!(
        jar.get_http_cookie_header(&url("https://example.test/login/en"), 0)
            .as_deref(),
        Some("sid=good; sid=broad")
    );
    assert_eq!(
        jar.get_http_cookie_header(&url("http://example.test/login/en"), 0)
            .as_deref(),
        Some("sid=broad")
    );
}

#[test]
fn secure_integrity_checks_overlapping_parent_and_subdomains() {
    let mut jar = CookieJar::new();
    assert!(jar.store_set_cookie(
        "sid=parent; Domain=example.test; Path=/; Secure",
        &url("https://www.example.test/"),
        0,
    ));

    assert!(!jar.store_set_cookie(
        "sid=sub; Path=/",
        &url("http://sub.example.test/"),
        0,
    ));

    assert_eq!(
        jar.get_http_cookie_header(&url("https://sub.example.test/"), 0)
            .as_deref(),
        Some("sid=parent")
    );
}

#[test]
fn unrelated_domain_and_different_cookie_name_do_not_trigger_integrity_block() {
    let mut jar = CookieJar::new();
    seed_secure(&mut jar, "/");

    assert!(jar.store_set_cookie(
        "other=1; Path=/",
        &url("http://example.test/"),
        0,
    ));
    assert!(jar.store_set_cookie(
        "sid=elsewhere; Path=/",
        &url("http://other.test/"),
        0,
    ));

    assert_eq!(
        jar.get_http_cookie_header(&url("http://example.test/"), 0)
            .as_deref(),
        Some("other=1")
    );
    assert_eq!(
        jar.get_http_cookie_header(&url("http://other.test/"), 0)
            .as_deref(),
        Some("sid=elsewhere")
    );
}

#[test]
fn insecure_document_cookie_cannot_overlay_secure_state_but_https_can() {
    let mut jar = CookieJar::new();
    let secure_source = url("https://example.test/");
    assert!(jar.store_set_cookie(
        "sid=good; Path=/; Secure",
        &secure_source,
        0,
    ));

    jar.set_document_cookie(
        "sid=script-http; Path=/",
        &url("http://example.test/"),
        0,
    );
    assert_eq!(
        jar.get_http_cookie_header(&secure_source, 0).as_deref(),
        Some("sid=good")
    );

    // HttpOnly is intentionally absent here. This test isolates the Secure
    // overlay rule; #111 separately proves that script can never overwrite an
    // HttpOnly cookie, even from HTTPS.
    jar.set_document_cookie(
        "sid=script-https; Path=/",
        &secure_source,
        0,
    );
    assert_eq!(
        jar.get_http_cookie_header(&url("http://example.test/"), 0)
            .as_deref(),
        Some("sid=script-https")
    );
}

#[test]
fn cookie_network_rejects_insecure_set_cookie_before_it_reaches_shared_jar() {
    let inner = Rc::new(ManualNetwork::new());
    inner.set_auto_complete(true);

    let response_url_text = "http://example.test/login";
    let response_url = url(response_url_text);
    let mut response = FetchResponse::synthetic(
        response_url.clone(),
        200,
        Some("text/plain"),
        b"ok".to_vec(),
    );
    response
        .headers
        .append_raw("set-cookie", "sid=network-evil; Path=/login");
    inner.respond(response_url_text, response);

    let mut seeded = CookieJar::new();
    seed_secure(&mut seeded, "/login");
    let jar = Rc::new(RefCell::new(seeded));
    let network = CookieNetwork::new(
        inner.clone(),
        jar.clone(),
        Rc::new(ManualClock::new()),
    );

    network.start(7, FetchRequest::get(response_url));
    let completions = network.poll();
    assert_eq!(completions.len(), 1);
    let delivered = completions[0].result.as_ref().expect("response delivered");
    assert!(
        delivered.headers.get("set-cookie").is_none(),
        "Set-Cookie stays hidden even when storage rejects it"
    );

    assert_eq!(
        jar.borrow()
            .get_http_cookie_header(&url("https://example.test/login"), 0)
            .as_deref(),
        Some("sid=good")
    );
}

#[test]
fn secure_https_response_can_replace_existing_secure_cookie() {
    let mut jar = CookieJar::new();
    seed_secure(&mut jar, "/login");
    let secure_source = url("https://example.test/login");

    assert!(jar.store_set_cookie("sid=new; Path=/login", &secure_source, 0));
    assert_eq!(
        jar.get_http_cookie_header(&secure_source, 0).as_deref(),
        Some("sid=new")
    );
}
