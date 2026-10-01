use browser_engine::html::parse_html;
use browser_engine::net::FetchResponse;
use browser_engine::{DocumentReferrerContext, ReferrerPolicy, Url};

fn response(policy: &str) -> FetchResponse {
    let mut response = FetchResponse::synthetic(
        Url::parse("https://source.test/private/page?q=1").unwrap(),
        200,
        Some("text/html"),
        Vec::new(),
    );
    response.headers.append_raw("referrer-policy", policy);
    response
}

#[test]
fn inert_template_meta_does_not_override_committed_document_policy() {
    let response = response("origin");
    let dom = parse_html(
        r#"<html><head>
            <template>
                <meta name="referrer" content="no-referrer">
            </template>
        </head></html>"#,
    );

    let context = DocumentReferrerContext::from_response_and_document(&response, &dom);
    assert_eq!(context.policy(), ReferrerPolicy::Origin);

    let target = Url::parse("https://other.test/next").unwrap();
    assert_eq!(
        context
            .policy()
            .compute(context.source().unwrap(), &target)
            .as_deref(),
        Some("https://source.test/")
    );
}

#[test]
fn template_boundary_does_not_hide_following_live_meta() {
    let response = response("unsafe-url");
    let dom = parse_html(
        r#"<html><head>
            <meta name="referrer" content="origin">
            <template>
                <meta name="referrer" content="no-referrer">
            </template>
            <meta name="referrer" content="strict-origin">
        </head></html>"#,
    );

    let context = DocumentReferrerContext::from_response_and_document(&response, &dom);
    assert_eq!(context.policy(), ReferrerPolicy::StrictOrigin);
}
