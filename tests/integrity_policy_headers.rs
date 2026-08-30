use browser_engine::{
    IntegrityPolicyContainer, IntegrityPolicyDestination, IntegrityPolicyRequestMode,
};
use browser_engine::net::{FetchResponse, HeaderMap};
use browser_engine::Url;

#[test]
fn absent_response_headers_do_not_create_a_document_policy() {
    let container = IntegrityPolicyContainer::from_headers(&HeaderMap::new());
    assert!(container.enforced.sources.is_empty());
    assert!(container.enforced.blocked_destinations.is_empty());
    assert!(container.report_only.sources.is_empty());
}

#[test]
fn response_headers_commit_enforced_and_report_only_policy_separately() {
    let mut response = FetchResponse::synthetic(
        Url::parse("https://example.test/index.html").unwrap(),
        200,
        Some("text/html"),
        Vec::new(),
    );
    response.headers.insert_raw(
        "Integrity-Policy",
        "blocked-destinations=(script), endpoints=(enforce)",
    );
    response.headers.insert_raw(
        "Integrity-Policy-Report-Only",
        "blocked-destinations=(style), endpoints=(observe)",
    );

    let container = IntegrityPolicyContainer::from_response(&response);
    let script = container.evaluate(
        IntegrityPolicyDestination::Script,
        false,
        IntegrityPolicyRequestMode::NoCors,
        false,
    );
    assert!(script.blocked);
    assert!(script.enforced_violation);
    assert!(!script.report_only_violation);

    let style = container.evaluate(
        IntegrityPolicyDestination::Style,
        false,
        IntegrityPolicyRequestMode::NoCors,
        false,
    );
    assert!(!style.blocked);
    assert!(!style.enforced_violation);
    assert!(style.report_only_violation);
}

#[test]
fn valid_cors_integrity_short_circuits_a_response_committed_policy() {
    let mut headers = HeaderMap::new();
    headers.insert_raw("integrity-policy", "blocked-destinations=(script)");
    let container = IntegrityPolicyContainer::from_headers(&headers);

    let decision = container.evaluate(
        IntegrityPolicyDestination::Script,
        true,
        IntegrityPolicyRequestMode::Cors,
        false,
    );
    assert!(!decision.blocked);
    assert!(!decision.enforced_violation);
}

#[test]
fn duplicate_policy_lines_do_not_accidentally_widen_enforcement() {
    let mut headers = HeaderMap::new();
    headers.append_raw("integrity-policy", "blocked-destinations=(script)");
    headers.append_raw("integrity-policy", "blocked-destinations=(style)");
    let container = IntegrityPolicyContainer::from_headers(&headers);

    assert!(!container
        .enforced
        .blocks_destination(IntegrityPolicyDestination::Script));
    assert!(!container
        .enforced
        .blocks_destination(IntegrityPolicyDestination::Style));
}
