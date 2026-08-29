use browser_engine::net::{FetchRequest, FetchResponse, Url};
use browser_engine::{
    prepare_subresource_request, subresource_redirect_state, subresource_referrer_policy,
    ReferrerPolicy,
};

fn url(input: &str) -> Url {
    Url::parse(input).unwrap()
}

#[test]
fn image_style_override_can_send_full_referrer_cross_origin() {
    let source = url("https://page.test/private/gallery?album=1#preview");
    let mut request = FetchRequest::get(url("https://images.test/photo.png"));

    prepare_subresource_request(
        &mut request,
        Some(&source),
        ReferrerPolicy::StrictOriginWhenCrossOrigin,
        Some("unsafe-url"),
    );

    assert_eq!(
        request.headers.get("referer").as_deref(),
        Some("https://page.test/private/gallery?album=1")
    );
}

#[test]
fn missing_or_invalid_attribute_inherits_document_policy() {
    assert_eq!(
        subresource_referrer_policy(ReferrerPolicy::Origin, None),
        ReferrerPolicy::Origin
    );
    assert_eq!(
        subresource_referrer_policy(ReferrerPolicy::SameOrigin, Some(" unsafe-url ")),
        ReferrerPolicy::SameOrigin
    );
}

#[test]
fn no_referrer_attribute_suppresses_even_a_stale_authored_header() {
    let source = url("https://page.test/private/index.html");
    let mut request = FetchRequest::get(url("https://cdn.test/app.js"));
    request
        .headers
        .insert_raw("referer", "https://forged.invalid/value");

    prepare_subresource_request(
        &mut request,
        Some(&source),
        ReferrerPolicy::UnsafeUrl,
        Some("no-referrer"),
    );

    assert_eq!(request.headers.get("referer"), None);
}

#[test]
fn redirect_response_can_tighten_element_selected_policy_for_next_hop() {
    let source = url("https://page.test/private/index.html?q=1#fragment");
    let mut state = subresource_redirect_state(
        Some(&source),
        ReferrerPolicy::StrictOriginWhenCrossOrigin,
        Some("unsafe-url"),
    );

    let mut first = FetchRequest::get(url("https://cdn.test/start.js"));
    state.prepare_request(&mut first);
    assert_eq!(
        first.headers.get("referer").as_deref(),
        Some("https://page.test/private/index.html?q=1")
    );

    let mut redirect = FetchResponse::synthetic(
        url("https://cdn.test/start.js"),
        302,
        None,
        Vec::new(),
    );
    redirect
        .headers
        .append_raw("referrer-policy", "no-referrer");
    state.observe_redirect_response(&redirect);

    let mut second = FetchRequest::get(url("https://static.test/final.js"));
    second
        .headers
        .insert_raw("referer", "https://stale.invalid/previous-hop");
    state.prepare_request(&mut second);

    assert_eq!(state.policy(), ReferrerPolicy::NoReferrer);
    assert_eq!(second.headers.get("referer"), None);
}
