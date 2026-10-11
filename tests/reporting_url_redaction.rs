use browser_engine::integrity_policy::{
    IntegrityPolicy, IntegrityPolicyDecision, IntegrityPolicyDestination,
};
use browser_engine::integrity_policy_reporting::build_integrity_violation_reports;
use browser_engine::net::Url;

fn violating_policy() -> (IntegrityPolicy, IntegrityPolicyDecision) {
    (
        IntegrityPolicy::parse("blocked-destinations=(script), endpoints=(collector)"),
        IntegrityPolicyDecision {
            blocked: true,
            enforced_violation: true,
            report_only_violation: false,
        },
    )
}

#[test]
fn non_http_report_urls_expose_only_the_scheme() {
    let (enforced, decision) = violating_policy();
    let empty = IntegrityPolicy::default();

    let reports = build_integrity_violation_reports(
        &Url::parse("file:///home/alice/private/index.html#account-token").unwrap(),
        &Url::parse("data:text/javascript,console.log('secret')#payload").unwrap(),
        IntegrityPolicyDestination::Script,
        decision,
        &enforced,
        &empty,
    );

    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].body.document_url, "file");
    assert_eq!(reports[0].body.blocked_url, "data");
    assert!(!reports[0].body.document_url.contains("alice"));
    assert!(!reports[0].body.blocked_url.contains("secret"));
}

#[test]
fn http_report_urls_keep_path_and_query_but_drop_fragments() {
    let (enforced, decision) = violating_policy();
    let empty = IntegrityPolicy::default();

    let reports = build_integrity_violation_reports(
        &Url::parse("https://example.test/page?view=full#private").unwrap(),
        &Url::parse("http://cdn.test/app.js?v=7#module-state").unwrap(),
        IntegrityPolicyDestination::Script,
        decision,
        &enforced,
        &empty,
    );

    assert_eq!(reports.len(), 1);
    assert_eq!(
        reports[0].body.document_url,
        "https://example.test/page?view=full"
    );
    assert_eq!(reports[0].body.blocked_url, "http://cdn.test/app.js?v=7");
}

#[test]
fn redaction_is_shared_by_enforced_and_report_only_reports() {
    let enforced = IntegrityPolicy::parse(
        "blocked-destinations=(script), endpoints=(enforced-endpoint)",
    );
    let report_only = IntegrityPolicy::parse(
        "blocked-destinations=(script), endpoints=(observe-endpoint)",
    );
    let decision = IntegrityPolicyDecision {
        blocked: true,
        enforced_violation: true,
        report_only_violation: true,
    };

    let reports = build_integrity_violation_reports(
        &Url::parse("demo://internal/private/page#state").unwrap(),
        &Url::parse("file:///tmp/secret-script.js#fragment").unwrap(),
        IntegrityPolicyDestination::Script,
        decision,
        &enforced,
        &report_only,
    );

    assert_eq!(reports.len(), 2);
    for report in reports {
        assert_eq!(report.body.document_url, "demo");
        assert_eq!(report.body.blocked_url, "file");
    }
}
