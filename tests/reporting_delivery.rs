use browser_engine::{
    batch_resolved_integrity_reports, resolve_integrity_violation_reports, IntegrityViolationReport,
    IntegrityViolationReportBody, ReportingEndpoints, REPORTING_CONTENT_TYPE,
};

fn report(endpoint: &str, blocked: &str, report_only: bool) -> IntegrityViolationReport {
    IntegrityViolationReport {
        report_type: "integrity-violation",
        endpoint: endpoint.to_string(),
        body: IntegrityViolationReportBody {
            document_url: "https://example.test/page".to_string(),
            blocked_url: blocked.to_string(),
            destination: "script".to_string(),
            report_only,
        },
    }
}

#[test]
fn resolves_then_batches_multiple_policy_names_sharing_one_url() {
    let endpoints = ReportingEndpoints::parse(
        r#"primary="https://reports.test/collect", observe="https://reports.test/collect""#,
    );
    let reports = vec![
        report("primary", "https://cdn.test/app.js", false),
        report("observe", "https://cdn.test/app.js", true),
    ];

    let resolved = resolve_integrity_violation_reports(&reports, &endpoints);
    let batches = batch_resolved_integrity_reports(&resolved);

    assert_eq!(REPORTING_CONTENT_TYPE, "application/reports+json");
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].len(), 2);
    assert_eq!(batches[0].endpoint_url.to_string(), "https://reports.test/collect");

    let json = batches[0].to_json(42, "toy-browser-test");
    assert_eq!(json.matches("\"type\":\"integrity-violation\"").count(), 2);
    assert!(json.contains("\"reportOnly\":false"));
    assert!(json.contains("\"reportOnly\":true"));
}

#[test]
fn unresolved_endpoint_never_enters_a_delivery_batch() {
    let endpoints = ReportingEndpoints::parse(
        r#"known="https://reports.test/collect""#,
    );
    let reports = vec![report("missing", "https://cdn.test/app.js", false)];

    let resolved = resolve_integrity_violation_reports(&reports, &endpoints);
    assert!(resolved.is_empty());
    assert!(batch_resolved_integrity_reports(&resolved).is_empty());
}

#[test]
fn distinct_concrete_destinations_remain_separate_posts() {
    let endpoints = ReportingEndpoints::parse(
        r#"a="https://reports.test/a", b="https://reports.test/b""#,
    );
    let reports = vec![
        report("a", "https://cdn.test/one.js", false),
        report("b", "https://cdn.test/two.js", false),
        report("a", "https://cdn.test/three.js", false),
    ];

    let resolved = resolve_integrity_violation_reports(&reports, &endpoints);
    let batches = batch_resolved_integrity_reports(&resolved);

    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0].endpoint_url.to_string(), "https://reports.test/a");
    assert_eq!(batches[0].len(), 2);
    assert_eq!(batches[1].endpoint_url.to_string(), "https://reports.test/b");
    assert_eq!(batches[1].len(), 1);
}
