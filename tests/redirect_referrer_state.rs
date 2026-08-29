use browser_engine::net::{FetchRequest, FetchResponse, Url};
use browser_engine::{RedirectPlanner, RedirectReferrerState, ReferrerPolicy};

fn url(input: &str) -> Url {
    Url::parse(input).unwrap()
}

fn redirect(url_value: &str, policy: &str) -> FetchResponse {
    let mut response = FetchResponse::synthetic(
        url("https://redirect.test/hop"),
        302,
        None,
        Vec::new(),
    );
    response.headers.append_raw("location", url_value);
    response.headers.append_raw("referrer-policy", policy);
    response
}

#[test]
fn redirect_state_survives_origin_only_serialization_and_can_recompute_from_source() {
    let source = url("https://source.test/private/path?q=secret#fragment");
    let mut state = RedirectReferrerState::new(
        Some(source),
        ReferrerPolicy::StrictOriginWhenCrossOrigin,
    );
    let mut planner = RedirectPlanner::default();

    let mut first = FetchRequest::get(url("https://redirect.test/start"));
    state.prepare_request(&mut first);
    assert_eq!(first.headers.get("referer"), Some("https://source.test/"));

    // First redirect keeps only the origin. The state must not replace its
    // stable source URL with that serialized header value.
    let first_response = redirect("https://source.test/landing", "same-origin");
    state.observe_redirect_response(&first_response);
    let mut second = planner
        .next_request(&first, &first_response)
        .unwrap()
        .unwrap();
    state.prepare_request(&mut second);

    // Because the target is same-origin with the original source, the full
    // source URL is available again under same-origin policy.
    assert_eq!(
        second.headers.get("referer"),
        Some("https://source.test/private/path?q=secret")
    );
}

#[test]
fn no_referrer_redirect_removes_previous_hop_header_before_dispatch() {
    let source = url("https://source.test/private/path?q=secret#fragment");
    let mut state = RedirectReferrerState::new(Some(source), ReferrerPolicy::UnsafeUrl);
    let mut planner = RedirectPlanner::default();

    let mut first = FetchRequest::get(url("https://redirect.test/start"));
    state.prepare_request(&mut first);
    assert!(first.headers.has("referer"));

    let response = redirect("https://target.test/next", "unsafe-url, no-referrer");
    state.observe_redirect_response(&response);
    let mut next = planner.next_request(&first, &response).unwrap().unwrap();

    // Planner clears the stale previous-hop value and the referrer state then
    // confirms that the updated policy emits no replacement.
    state.prepare_request(&mut next);
    assert_eq!(state.policy(), ReferrerPolicy::NoReferrer);
    assert!(!next.headers.has("referer"));
}

#[test]
fn unknown_policy_tokens_do_not_destroy_existing_redirect_policy() {
    let mut state = RedirectReferrerState::new(
        Some(url("https://source.test/page")),
        ReferrerPolicy::Origin,
    );
    let response = redirect(
        "https://target.test/next",
        "future-policy, another-extension",
    );

    state.observe_redirect_response(&response);
    assert_eq!(state.policy(), ReferrerPolicy::Origin);

    let mut request = FetchRequest::get(url("https://target.test/next"));
    state.prepare_request(&mut request);
    assert_eq!(request.headers.get("referer"), Some("https://source.test/"));
}
