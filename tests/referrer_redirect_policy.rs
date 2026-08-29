use browser_engine::net::{FetchRequest, FetchResponse, HeaderMap, Method, Url};
use browser_engine::{RedirectPlanner, ReferrerPolicy};

fn url(input: &str) -> Url {
    Url::parse(input).unwrap()
}

fn redirect_response(fields: &[&str]) -> FetchResponse {
    let mut response = FetchResponse::synthetic(
        url("https://redirect.test/hop"),
        302,
        None,
        Vec::new(),
    );
    response
        .headers
        .append_raw("location", "https://target.test/next");
    for value in fields {
        response.headers.append_raw("referrer-policy", value);
    }
    response
}

#[test]
fn redirect_response_uses_last_recognized_policy_across_raw_fields() {
    let response = redirect_response(&[
        "no-referrer, future-policy",
        "origin-when-cross-origin, strict-origin",
    ]);

    assert_eq!(
        ReferrerPolicy::from_response(&response),
        Some(ReferrerPolicy::StrictOrigin)
    );
}

#[test]
fn unknown_redirect_policy_preserves_the_existing_request_policy() {
    let response = redirect_response(&["future-policy", "another-extension"]);

    assert_eq!(
        ReferrerPolicy::Origin.updated_on_redirect(&response),
        ReferrerPolicy::Origin
    );
}

#[test]
fn redirect_policy_composes_with_planner_before_recomputing_referer() {
    let source_document = url("https://source.test/private/page?q=1#secret");
    let mut headers = HeaderMap::new();
    headers.insert_raw("referer", "https://source.test/private/page?q=1");
    let request = FetchRequest::new(
        url("https://redirect.test/start"),
        Method::Get,
        headers,
        None,
    );
    let response = redirect_response(&["unsafe-url", "no-referrer"]);

    let policy = ReferrerPolicy::UnsafeUrl.updated_on_redirect(&response);
    let mut planner = RedirectPlanner::default();
    let next = planner
        .next_request(&request, &response)
        .expect("valid redirect")
        .expect("next hop");

    // RedirectPlanner must discard the Referer computed for the previous hop.
    assert!(!next.headers.has("referer"));
    // The response's final recognized policy controls recomputation for the
    // next hop, so no referrer is emitted despite the previous unsafe-url mode.
    assert_eq!(policy, ReferrerPolicy::NoReferrer);
    assert_eq!(policy.compute(&source_document, &next.url), None);
}

#[test]
fn later_origin_policy_can_replace_an_earlier_no_referrer_fallback() {
    let source_document = url("https://source.test/private/page?q=1#secret");
    let response = redirect_response(&["no-referrer", "future-policy, origin"]);
    let policy = ReferrerPolicy::NoReferrer.updated_on_redirect(&response);

    assert_eq!(policy, ReferrerPolicy::Origin);
    assert_eq!(
        policy.compute(&source_document, &url("https://target.test/next")),
        Some("https://source.test/".to_string())
    );
}
