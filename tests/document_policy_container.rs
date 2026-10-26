use browser_engine::net::HeaderMap;
use browser_engine::{CorpOriginRelation, CrossOriginEmbedderPolicy, DocumentPolicyContainer};

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

#[test]
fn valid_structured_parameters_preserve_enforced_and_report_only_policy() {
    for (value, endpoint) in [
        ("require-corp; flag", None),
        ("require-corp; report-to", None),
        ("require-corp; report-to=?1", None),
        ("require-corp; report-to=?0", None),
        ("require-corp; report-to=endpoint", None),
        ("require-corp; report-to=7", None),
        ("require-corp; report-to=:Zm9v:", None),
        ("require-corp; future=@123", None),
        (r#"require-corp; future=%"caf%c3%a9""#, None),
        (r#"require-corp; future=%"ends\""#, None),
        (
            r#"require-corp; report-to="first"; report-to="last""#,
            Some("last"),
        ),
        (r#"require-corp; report-to="obsolete"; report-to"#, None),
    ] {
        let document = headers(&[
            ("cross-origin-embedder-policy", value),
            (
                "cross-origin-embedder-policy-report-only",
                "require-corp; observe",
            ),
        ]);
        let container = DocumentPolicyContainer::from_response_headers(&document);
        assert_eq!(
            container.embedder_policy.policy,
            CrossOriginEmbedderPolicy::RequireCorp,
            "{value}"
        );
        assert_eq!(
            container.embedder_policy.report_to.as_deref(),
            endpoint,
            "{value}"
        );
        let result = container.check_no_cors_response(
            &HeaderMap::new(),
            CorpOriginRelation::CrossSite,
            true,
            true,
            false,
            false,
        );
        assert!(
            !result.allowed,
            "valid parameters must not disable enforced policy: {value}"
        );
        assert!(
            result.report_only_violation,
            "valid flag must preserve report-only policy: {value}"
        );
        let opted_in = headers(&[("cross-origin-resource-policy", "cross-origin")]);
        let allowed = container.check_no_cors_response(
            &opted_in,
            CorpOriginRelation::CrossSite,
            true,
            true,
            false,
            false,
        );
        assert!(allowed.allowed);
        assert!(!allowed.report_only_violation);
    }
}
