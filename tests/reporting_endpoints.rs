use browser_engine::net::{FetchResponse, HeaderMap};
use browser_engine::{
    build_integrity_violation_reports, evaluate_integrity_policy,
    resolve_integrity_violation_reports, IntegrityPolicy, IntegrityPolicyDestination,
    IntegrityPolicyRequestMode, ReportingEndpoints, Url, REPORTING_ENDPOINTS_HEADER,
};

#[test]
fn response_header_maps_named_reporting_endpoints() {
    let mut response = FetchResponse::synthetic(
        Url::parse("https://example.test/page").unwrap(),
        200,
        Some("text/html"),
        Vec::new(),
    );
    response.headers.insert_raw(
        REPORTING_ENDPOINTS_HEADER,
        r#"primary="https://reports.test/integrity", insecure="http://reports.test/plain""#,
    );

    let endpoints = ReportingEndpoints::from_response(&response);
    assert_eq!(endpoints.len(), 1);
    assert_eq!(
        endpoints.get("primary").unwrap().to_string(),
        "https://reports.test/integrity"
    );
    assert!(endpoints.get("insecure").is_none());
}

#[test]
fn integrity_reports_resolve_only_declared_secure_endpoint_names() {
    let enforced = IntegrityPolicy::parse(
        "blocked-destinations=(script), endpoints=(primary missing)",
    );
    let report_only = IntegrityPolicy::parse(
        "blocked-destinations=(script), endpoints=(observe)",
    );
    let decision = evaluate_integrity_policy(
        &enforced,
        &report_only,
        IntegrityPolicyDestination::Script,
        false,
        IntegrityPolicyRequestMode::NoCors,
        false,
    );
    let reports = build_integrity_violation_reports(
        &Url::parse("https://example.test/page").unwrap(),
        &Url::parse("https://cdn.test/app.js").unwrap(),
        IntegrityPolicyDestination::Script,
        decision,
        &enforced,
        &report_only,
    );
    assert_eq!(reports.len(), 3);

    let endpoints = ReportingEndpoints::parse(
        r#"primary="https://reports.test/enforced", observe="https://reports.test/observe""#,
    );
    let resolved = resolve_integrity_violation_reports(&reports, &endpoints);

    assert_eq!(resolved.len(), 2);
    assert_eq!(resolved[0].endpoint_name, "primary");
    assert_eq!(
        resolved[0].endpoint_url.to_string(),
        "https://reports.test/enforced"
    );
    assert!(!resolved[0].report.body.report_only);
    assert_eq!(resolved[1].endpoint_name, "observe");
    assert!(resolved[1].report.body.report_only);
}

#[test]
fn repeated_dictionary_members_fail_closed_after_header_combination() {
    let mut headers = HeaderMap::new();
    headers.append_raw(
        REPORTING_ENDPOINTS_HEADER,
        r#"primary="https://reports.test/one""#,
    );
    headers.append_raw(
        REPORTING_ENDPOINTS_HEADER,
        r#"primary="https://reports.test/two""#,
    );

    assert!(ReportingEndpoints::from_headers(&headers).is_empty());
}

#[test]
fn invalid_dictionary_cannot_redirect_integrity_reports() {
    let endpoints = ReportingEndpoints::parse(
        r#"primary="https://reports.test/good", broken=https://attacker.test/report"#,
    );
    assert!(endpoints.is_empty());

    let policy = IntegrityPolicy::parse(
        "blocked-destinations=(style), endpoints=(primary)",
    );
    let reports = build_integrity_violation_reports(
        &Url::parse("https://example.test/").unwrap(),
        &Url::parse("https://cdn.test/app.css").unwrap(),
        IntegrityPolicyDestination::Style,
        browser_engine::IntegrityPolicyDecision {
            blocked: true,
            enforced_violation: true,
            report_only_violation: false,
        },
        &policy,
        &IntegrityPolicy::default(),
    );

    assert!(resolve_integrity_violation_reports(&reports, &endpoints).is_empty());
}