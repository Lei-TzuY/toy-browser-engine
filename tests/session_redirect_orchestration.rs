use std::rc::Rc;

use browser_engine::cookie_network::{CookieCredentials, CookieRequestPolicy};
use browser_engine::cookie_same_site::SameSiteRequestContext;
use browser_engine::eventloop::ManualClock;
use browser_engine::net::{
    FetchError, FetchRequest, FetchResponse, ManualNetwork, Method, NetworkBackend, Url,
};
use browser_engine::SessionNetwork;

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn redirect_response(source: &str, location: &str) -> FetchResponse {
    let mut response = FetchResponse::synthetic(url(source), 302, Some("text/plain"), Vec::new());
    response.headers.insert_raw("location", location);
    response
}

#[test]
fn intermediate_cookie_is_absorbed_before_the_next_hop() {
    let clock = Rc::new(ManualClock::new());
    let transport = Rc::new(ManualNetwork::new());
    let network = SessionNetwork::with_new_state_redirecting(transport.clone(), clock);

    let mut first = redirect_response("http://example.test/start", "/next");
    first.headers.append_raw("set-cookie", "hop=one; Path=/");
    transport.respond("http://example.test/start", first);
    transport.respond_text("http://example.test/next", "done");

    network.start(1, FetchRequest::get(url("http://example.test/start")));
    assert_eq!(transport.requests().len(), 1);
    assert!(transport.requests()[0].headers.get("cookie").is_none());

    assert!(transport.complete(1));
    assert!(
        network.poll().is_empty(),
        "an intermediate redirect must stay inside the redirect chain"
    );

    let requests = transport.requests();
    assert_eq!(
        requests.len(),
        2,
        "the next hop should be dispatched immediately"
    );
    assert_eq!(requests[1].url.to_string(), "http://example.test/next");
    assert_eq!(
        requests[1].headers.get("cookie").as_deref(),
        Some("hop=one"),
        "Set-Cookie from the redirect response must be visible to the next hop"
    );

    assert!(transport.complete(1));
    let completions = network.poll();
    assert_eq!(completions.len(), 1);
    let response = completions[0].result.as_ref().expect("final response");
    assert_eq!(response.status, 200);
    assert!(response.redirected);
}

#[test]
fn intermediate_hsts_upgrade_precedes_secure_cookie_selection() {
    let clock = Rc::new(ManualClock::new());
    let transport = Rc::new(ManualNetwork::new());
    let network = SessionNetwork::with_new_state_redirecting(transport.clone(), clock);

    let mut first = redirect_response("https://example.test/start", "http://example.test/next");
    first
        .headers
        .append_raw("strict-transport-security", "max-age=3600");
    first
        .headers
        .append_raw("set-cookie", "securehop=one; Path=/; Secure");
    transport.respond("https://example.test/start", first);
    transport.respond_text("https://example.test/next", "secure done");

    network.start(2, FetchRequest::get(url("https://example.test/start")));
    assert!(transport.complete(2));
    assert!(network.poll().is_empty());

    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].url.to_string(),
        "https://example.test/next",
        "STS learned from the intermediate response must upgrade Location before transport"
    );
    assert_eq!(
        requests[1].headers.get("cookie").as_deref(),
        Some("securehop=one"),
        "Secure cookie selection must observe the HSTS-upgraded next-hop URL"
    );

    assert!(transport.complete(2));
    let completions = network.poll();
    assert_eq!(completions.len(), 1);
    assert!(completions[0].result.as_ref().unwrap().redirected);
}

#[test]
fn cross_origin_redirect_is_blocked_before_second_transport_dispatch() {
    let clock = Rc::new(ManualClock::new());
    let transport = Rc::new(ManualNetwork::new());
    let network = SessionNetwork::with_new_state_redirecting(transport.clone(), clock);

    transport.respond(
        "http://example.test/start",
        redirect_response("http://example.test/start", "http://other.test/next"),
    );

    network.start(3, FetchRequest::get(url("http://example.test/start")));
    assert!(transport.complete(3));
    let completions = network.poll();

    assert_eq!(completions.len(), 1);
    assert!(matches!(completions[0].result, Err(FetchError::Blocked(_))));
    assert_eq!(
        transport.requests().len(),
        1,
        "same-origin Fetch must reject Location before another origin sees a request"
    );
    assert!(network.cookie_policy_registry().is_empty());
}

#[test]
fn credentials_omit_is_rearmed_for_every_redirect_hop() {
    let clock = Rc::new(ManualClock::new());
    let transport = Rc::new(ManualNetwork::new());
    let network = SessionNetwork::with_new_state_redirecting(transport.clone(), clock);
    let origin = url("http://example.test/");

    assert!(network
        .cookie_jar()
        .borrow_mut()
        .store_set_cookie("existing=one; Path=/", &origin, 0,));
    network.cookie_policy_registry().set(
        4,
        CookieRequestPolicy::new(
            CookieCredentials::Omit,
            SameSiteRequestContext::same_site(Method::Get),
        ),
    );

    let mut first = redirect_response("http://example.test/start", "/next");
    first
        .headers
        .append_raw("set-cookie", "newcookie=two; Path=/");
    transport.respond("http://example.test/start", first);
    transport.respond_text("http://example.test/next", "done");

    network.start(4, FetchRequest::get(url("http://example.test/start")));
    assert!(transport.requests()[0].headers.get("cookie").is_none());

    assert!(transport.complete(4));
    assert!(network.poll().is_empty());
    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1].headers.get("cookie").is_none(),
        "credentials=omit must survive CookieNetwork consuming the first-hop policy"
    );
    assert_eq!(
        network
            .cookie_jar()
            .borrow()
            .get_document_cookie(&origin, 0),
        "existing=one",
        "Set-Cookie from an omitted intermediate response must not enter the jar"
    );

    assert!(transport.complete(4));
    let completions = network.poll();
    assert_eq!(completions.len(), 1);
    assert!(completions[0].result.as_ref().unwrap().redirected);
    assert!(network.cookie_policy_registry().is_empty());
}
