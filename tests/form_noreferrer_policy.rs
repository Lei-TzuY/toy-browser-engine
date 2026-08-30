use browser_engine::net::{FetchRequest, Url};
use browser_engine::{
    form_redirect_state, form_referrer_policy, ReferrerPolicy,
};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

#[test]
fn form_noreferrer_suppresses_a_full_document_referrer() {
    let source = url("https://source.test/private/form?q=1#fragment");
    let mut request = FetchRequest::get(url("https://target.test/submit"));

    form_redirect_state(
        Some(&source),
        ReferrerPolicy::UnsafeUrl,
        Some("external noreferrer"),
    )
    .prepare_request(&mut request);

    assert!(request.headers.get("referer").is_none());
}

#[test]
fn ordinary_form_rel_inherits_the_committed_document_policy() {
    let source = url("https://source.test/private/form?q=1#fragment");
    let mut request = FetchRequest::get(url("https://target.test/submit"));

    form_redirect_state(
        Some(&source),
        ReferrerPolicy::UnsafeUrl,
        Some("external noopener"),
    )
    .prepare_request(&mut request);

    assert_eq!(
        request.headers.get("referer").as_deref(),
        Some("https://source.test/private/form?q=1")
    );
}

#[test]
fn form_rel_uses_html_ascii_whitespace_tokenization() {
    assert_eq!(
        form_referrer_policy(
            ReferrerPolicy::Origin,
            Some("noopener\tNoReFeRrEr\nexternal")
        ),
        ReferrerPolicy::NoReferrer
    );
    assert_eq!(
        form_referrer_policy(
            ReferrerPolicy::Origin,
            Some("noopener,noreferrer")
        ),
        ReferrerPolicy::Origin,
        "a comma is part of one rel token, not a separator"
    );
}

#[test]
fn form_only_policy_does_not_mutate_the_document_policy_value() {
    let committed = ReferrerPolicy::UnsafeUrl;
    assert_eq!(
        form_referrer_policy(committed, Some("noreferrer")),
        ReferrerPolicy::NoReferrer
    );
    assert_eq!(committed, ReferrerPolicy::UnsafeUrl);
}
