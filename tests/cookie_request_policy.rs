use std::cell::RefCell;
use std::rc::Rc;

use browser_engine::cookie::CookieJar;
use browser_engine::cookie_network::{
    CookieCredentials, CookieNetwork, CookieRequestPolicy,
};
use browser_engine::cookie_same_site::SameSiteRequestContext;
use browser_engine::eventloop::ManualClock;
use browser_engine::net::fetch::Method;
use browser_engine::net::{
    FetchRequest, FetchResponse, ManualNetwork, NetworkBackend, Url,
};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn seed_same_site_variants(jar: &mut CookieJar) {
    let source = url("https://example.test/");
    assert!(jar.store_set_cookie(
        "strict=1; Path=/; SameSite=Strict",
        &source,
        0,
    ));
    assert!(jar.store_set_cookie(
        "lax=1; Path=/; SameSite=Lax",
        &source,
        0,
    ));
    assert!(jar.store_set_cookie(
        "none=1; Path=/; SameSite=None; Secure",
        &source,
        0,
    ));
}

fn response_with_cookie(url_text: &str, cookie: &str) -> FetchResponse {
    let mut response = FetchResponse::synthetic(
        url(url_text),
        200,
        Some("text/plain"),
        b"ok".to_vec(),
    );
    response.headers.append_raw("set-cookie", cookie);
    response
}

#[test]
fn unregistered_requests_keep_backward_compatible_same_site_include_behavior() {
    let inner = Rc::new(ManualNetwork::new());
    inner.set_auto_complete(true);
    let jar = Rc::new(RefCell::new(CookieJar::new()));
    seed_same_site_variants(&mut jar.borrow_mut());
    let network = CookieNetwork::new(
        inner.clone(),
        jar,
        Rc::new(ManualClock::new()),
    );

    network.start(1, FetchRequest::get(url("https://example.test/data")));
    let sent = inner.requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].headers.get("cookie").as_deref(),
        Some("strict=1; lax=1; none=1")
    );
}

#[test]
fn cross_site_subresource_filters_strict_and_lax_per_cookie() {
    let inner = Rc::new(ManualNetwork::new());
    inner.set_auto_complete(true);
    let jar = Rc::new(RefCell::new(CookieJar::new()));
    seed_same_site_variants(&mut jar.borrow_mut());
    let network = CookieNetwork::new(
        inner.clone(),
        jar,
        Rc::new(ManualClock::new()),
    );

    network.set_request_policy(
        2,
        CookieRequestPolicy::new(
            CookieCredentials::Include,
            SameSiteRequestContext::cross_site_subresource(Method::Get),
        ),
    );
    network.start(2, FetchRequest::get(url("https://example.test/api")));

    assert_eq!(
        inner.requests()[0].headers.get("cookie").as_deref(),
        Some("none=1")
    );
}

#[test]
fn lax_cookie_is_restored_for_cross_site_top_level_safe_navigation() {
    let inner = Rc::new(ManualNetwork::new());
    inner.set_auto_complete(true);
    let jar = Rc::new(RefCell::new(CookieJar::new()));
    seed_same_site_variants(&mut jar.borrow_mut());
    let network = CookieNetwork::new(
        inner.clone(),
        jar,
        Rc::new(ManualClock::new()),
    );

    network.set_request_policy(
        3,
        CookieRequestPolicy::new(
            CookieCredentials::Include,
            SameSiteRequestContext::cross_site_navigation(Method::Get),
        ),
    );
    network.start(3, FetchRequest::get(url("https://example.test/landing")));

    assert_eq!(
        inner.requests()[0].headers.get("cookie").as_deref(),
        Some("lax=1; none=1")
    );
}

#[test]
fn credentials_omit_suppresses_outgoing_and_incoming_cookie_state() {
    let inner = Rc::new(ManualNetwork::new());
    inner.set_auto_complete(true);
    let endpoint = "https://example.test/api";
    inner.respond(
        endpoint,
        response_with_cookie(endpoint, "server=new; Path=/; SameSite=Lax"),
    );

    let jar = Rc::new(RefCell::new(CookieJar::new()));
    assert!(jar.borrow_mut().store_set_cookie(
        "session=old; Path=/",
        &url("https://example.test/"),
        0,
    ));
    let network = CookieNetwork::new(
        inner.clone(),
        jar.clone(),
        Rc::new(ManualClock::new()),
    );

    network.set_request_policy(
        9,
        CookieRequestPolicy::omit(SameSiteRequestContext::same_site(Method::Get)),
    );
    let mut request = FetchRequest::get(url(endpoint));
    request.headers.insert_raw("cookie", "forged=evil");
    network.start(9, request);

    assert!(
        inner.requests()[0].headers.get("cookie").is_none(),
        "omit must remove both forged and jar-derived Cookie headers"
    );

    let completions = network.poll();
    assert_eq!(completions.len(), 1);
    let delivered = completions[0].result.as_ref().expect("response succeeds");
    assert!(
        delivered.headers.get("set-cookie").is_none(),
        "Set-Cookie remains forbidden to script even when credentials omit storage"
    );
    assert_eq!(
        jar.borrow()
            .get_http_cookie_header(&url(endpoint), 0)
            .as_deref(),
        Some("session=old"),
        "response Set-Cookie must not mutate the session jar"
    );
    assert_eq!(network.pending_policy_count(), 0);
}

#[test]
fn include_policy_accepts_response_cookie_and_cleans_up_after_completion() {
    let inner = Rc::new(ManualNetwork::new());
    inner.set_auto_complete(true);
    let endpoint = "https://example.test/include";
    inner.respond(
        endpoint,
        response_with_cookie(endpoint, "accepted=yes; Path=/"),
    );
    let jar = Rc::new(RefCell::new(CookieJar::new()));
    let network = CookieNetwork::new(
        inner,
        jar.clone(),
        Rc::new(ManualClock::new()),
    );

    let policy = CookieRequestPolicy::same_site(Method::Get);
    assert_eq!(network.set_request_policy(11, policy), None);
    assert_eq!(network.request_policy(11), Some(policy));
    assert_eq!(network.pending_policy_count(), 1);

    network.start(11, FetchRequest::get(url(endpoint)));
    assert_eq!(network.pending_policy_count(), 1);
    network.poll();

    assert_eq!(network.pending_policy_count(), 0);
    assert_eq!(network.request_policy(11), None);
    assert_eq!(
        jar.borrow()
            .get_http_cookie_header(&url(endpoint), 0)
            .as_deref(),
        Some("accepted=yes")
    );
}

#[test]
fn cancellation_discards_pending_policy_without_touching_other_ids() {
    let inner = Rc::new(ManualNetwork::new());
    let network = CookieNetwork::with_new_jar(
        inner,
        Rc::new(ManualClock::new()),
    );

    let first = CookieRequestPolicy::omit(SameSiteRequestContext::same_site(Method::Get));
    let second = CookieRequestPolicy::new(
        CookieCredentials::Include,
        SameSiteRequestContext::cross_site_subresource(Method::Get),
    );
    network.set_request_policy(21, first);
    network.set_request_policy(22, second);
    assert_eq!(network.pending_policy_count(), 2);

    network.cancel(21);
    assert_eq!(network.request_policy(21), None);
    assert_eq!(network.request_policy(22), Some(second));
    assert_eq!(network.pending_policy_count(), 1);

    assert_eq!(network.clear_request_policy(22), Some(second));
    assert_eq!(network.pending_policy_count(), 0);
}
