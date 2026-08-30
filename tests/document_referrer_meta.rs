use browser_engine::html::parse_html;
use browser_engine::net::FetchResponse;
use browser_engine::{DocumentReferrerContext, ReferrerPolicy, Url};

fn response(url: &str, policy: Option<&str>) -> FetchResponse {
    let mut response =
        FetchResponse::synthetic(Url::parse(url).unwrap(), 200, Some("text/html"), Vec::new());
    if let Some(policy) = policy {
        response.headers.append_raw("referrer-policy", policy);
    }
    response
}

#[test]
fn meta_referrer_policy_overrides_the_http_header_after_parse() {
    let response = response("https://source.test/private/page?q=1", Some("origin"));
    let dom =
        parse_html(r#"<html><head><meta name="referrer" content="no-referrer"></head></html>"#);

    let context = DocumentReferrerContext::from_response_and_document(&response, &dom);
    assert_eq!(context.policy(), ReferrerPolicy::NoReferrer);

    let target = Url::parse("https://other.test/next").unwrap();
    assert_eq!(
        context.policy().compute(context.source().unwrap(), &target),
        None
    );
}

#[test]
fn later_valid_meta_wins_while_invalid_values_do_not_reset_policy() {
    let response = response("https://source.test/page", Some("same-origin"));
    let dom = parse_html(
        r#"<html><head>
            <meta name="referrer" content="never">
            <meta name="referrer" content="future-policy">
            <meta name="referrer" content="origin-when-crossorigin">
        </head></html>"#,
    );

    let context = DocumentReferrerContext::from_response_and_document(&response, &dom);
    assert_eq!(context.policy(), ReferrerPolicy::OriginWhenCrossOrigin);
}

#[test]
fn legacy_always_enables_full_referrer_and_still_strips_fragment() {
    let response = response(
        "https://source.test/private/page?q=1#secret",
        Some("no-referrer"),
    );
    let dom = parse_html(r#"<meta NAME="REFERRER" content="ALWAYS">"#);

    let context = DocumentReferrerContext::from_response_and_document(&response, &dom);
    let target = Url::parse("http://other.test/next").unwrap();
    assert_eq!(context.policy(), ReferrerPolicy::UnsafeUrl);
    assert_eq!(
        context
            .policy()
            .compute(context.source().unwrap(), &target)
            .as_deref(),
        Some("https://source.test/private/page?q=1")
    );
}

#[test]
fn meta_content_uses_html_exact_value_rules_not_http_list_or_whitespace_rules() {
    let response = response("https://source.test/page", Some("strict-origin"));
    let dom = parse_html(
        r#"<meta name="referrer" content=" no-referrer ">
           <meta name="referrer" content="origin,unsafe-url">"#,
    );

    let context = DocumentReferrerContext::from_response_and_document(&response, &dom);
    assert_eq!(context.policy(), ReferrerPolicy::StrictOrigin);
}

#[test]
fn legacy_default_resets_header_policy_to_the_user_agent_default() {
    let response = response("https://source.test/page", Some("unsafe-url"));
    let dom = parse_html(r#"<meta name="referrer" content="default">"#);

    let context = DocumentReferrerContext::from_response_and_document(&response, &dom);
    assert_eq!(context.policy(), ReferrerPolicy::default());
}
