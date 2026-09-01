use browser_engine::{
    build_integrity_violation_reports, evaluate_integrity_policy, IntegrityPolicy,
    IntegrityPolicyDestination, IntegrityPolicyRequestMode, Url, INTEGRITY_VIOLATION_REPORT_TYPE,
};

#[test]
fn enforced_and_report_only_violations_target_their_own_endpoints() {
    let enforced = IntegrityPolicy::parse(
        "blocked-destinations=(script), endpoints=(enforce-a enforce-b)",
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
        &Url::parse("https://site.test/page#private").unwrap(),
        &Url::parse("https://cdn.test/app.js#hash").unwrap(),
        IntegrityPolicyDestination::Script,
        decision,
        &enforced,
        &report_only,
    );

    assert_eq!(reports.len(), 3);
    assert_eq!(reports[0].report_type, INTEGRITY_VIOLATION_REPORT_TYPE);
    assert_eq!(reports[0].endpoint, "enforce-a");
    assert_eq!(reports[1].endpoint, "enforce-b");
    assert_eq!(reports[2].endpoint, "observe");
    assert!(!reports[0].body.report_only);
    assert!(!reports[1].body.report_only);
    assert!(reports[2].body.report_only);
    assert_eq!(reports[0].body.destination, "script");
    assert_eq!(reports[0].body.document_url, "https://site.test/page");
    assert_eq!(reports[0].body.blocked_url, "https://cdn.test/app.js");
}

#[test]
fn style_report_only_violation_is_observable_without_an_enforced_report() {
    let enforced = IntegrityPolicy::parse(
        "blocked-destinations=(script), endpoints=(enforce)",
    );
    let report_only = IntegrityPolicy::parse(
        "blocked-destinations=(style), endpoints=(style-observer)",
    );
    let decision = evaluate_integrity_policy(
        &enforced,
        &report_only,
        IntegrityPolicyDestination::Style,
        false,
        IntegrityPolicyRequestMode::NoCors,
        false,
    );
    assert!(!decision.blocked);
    assert!(decision.report_only_violation);

    let reports = build_integrity_violation_reports(
        &Url::parse("https://site.test/").unwrap(),
        &Url::parse("https://cdn.test/site.css").unwrap(),
        IntegrityPolicyDestination::Style,
        decision,
        &enforced,
        &report_only,
    );

    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].endpoint, "style-observer");
    assert_eq!(reports[0].body.destination, "style");
    assert!(reports[0].body.report_only);
}
