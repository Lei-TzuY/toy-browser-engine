use browser_engine::{
    CorpOriginRelation, CrossOriginEmbedderPolicy, DocumentPolicyContainer, HeaderMap,
};

fn headers(entries: &[(&str, &str)]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in entries {
        headers.append_raw(name, value);
    }
    headers
}

#[test]
fn container_carries_document_coep_into_no_cors_response_checks() {
    let document = headers(&[("Cross-Origin-Embedder-Policy", "require-corp")]);
    let container = DocumentPolicyContainer::from_response_headers(&document);

    assert_eq!(
        container.embedder_policy.policy,
        CrossOriginEmbedderPolicy::RequireCorp
    );

    let blocked = container.check_no_cors_response(
        &HeaderMap::new(),
        CorpOriginRelation::CrossSite,
        true,
        true,
        false,
        false,
    );
    assert!(!blocked.allowed);

    let opted_in = headers(&[("Cross-Origin-Resource-Policy", "cross-origin")]);
    let allowed = container.check_no_cors_response(
        &opted_in,
        CorpOriginRelation::CrossSite,
        true,
        true,
        false,
        false,
    );
    assert!(allowed.allowed);
}

#[test]
fn report_only_policy_observes_without_enforcing() {
    let document = headers(&[(
        "Cross-Origin-Embedder-Policy-Report-Only",
        "require-corp; report-to=\"coep-endpoint\"",
    )]);
    let container = DocumentPolicyContainer::from_response_headers(&document);

    let result = container.check_no_cors_response(
        &HeaderMap::new(),
        CorpOriginRelation::CrossSite,
        true,
        true,
        true,
        false,
    );

    assert!(result.allowed);
    assert!(result.report_only_violation);
    assert_eq!(
        container.embedder_policy_report_only.report_to.as_deref(),
        Some("coep-endpoint")
    );
}

#[test]
fn secure_transport_guard_is_preserved_through_policy_container() {
    let document = headers(&[("Cross-Origin-Embedder-Policy", "unsafe-none")]);
    let resource = headers(&[("Cross-Origin-Resource-Policy", "same-site")]);
    let container = DocumentPolicyContainer::from_response_headers(&document);

    let result = container.check_no_cors_response(
        &resource,
        CorpOriginRelation::SameSite,
        false,
        true,
        false,
        false,
    );

    assert!(!result.allowed);
}

#[test]
fn credentialless_differs_for_credentialed_and_uncredentialed_subresources() {
    let document = headers(&[("Cross-Origin-Embedder-Policy", "credentialless")]);
    let container = DocumentPolicyContainer::from_response_headers(&document);

    let anonymous = container.check_no_cors_response(
        &HeaderMap::new(),
        CorpOriginRelation::CrossSite,
        true,
        true,
        false,
        false,
    );
    let credentialed = container.check_no_cors_response(
        &HeaderMap::new(),
        CorpOriginRelation::CrossSite,
        true,
        true,
        true,
        false,
    );

    assert!(anonymous.allowed);
    assert!(!credentialed.allowed);
}
