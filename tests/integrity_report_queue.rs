use browser_engine::{
    evaluate_integrity_policy, IntegrityPolicy, IntegrityPolicyDestination,
    IntegrityPolicyRequestMode, IntegrityReportQueue, Url, INTEGRITY_VIOLATION_REPORT_TYPE,
};

#[test]
fn queues_enforced_and_report_only_reports_for_delivery() {
    let enforced = IntegrityPolicy::parse(
        "blocked-destinations=(script), endpoints=(security backup)",
    );
    let report_only = IntegrityPolicy::parse(
        "blocked-destinations=(script), endpoints=(observer)",
    );
    let decision = evaluate_integrity_policy(
        &enforced,
        &report_only,
        IntegrityPolicyDestination::Script,
        false,
        IntegrityPolicyRequestMode::NoCors,
        false,
    );

    let mut queue = IntegrityReportQueue::new();
    assert_eq!(
        queue.enqueue_decision(
            &Url::parse("https://app.test/page#private").unwrap(),
            &Url::parse("https://cdn.test/app.js#module").unwrap(),
            IntegrityPolicyDestination::Script,
            decision,
            &enforced,
            &report_only,
        ),
        3
    );

    let first = queue.pop_front().unwrap();
    assert_eq!(first.report_type, INTEGRITY_VIOLATION_REPORT_TYPE);
    assert_eq!(first.endpoint, "security");
    assert_eq!(first.body.document_url, "https://app.test/page");
    assert_eq!(first.body.blocked_url, "https://cdn.test/app.js");
    assert!(!first.body.report_only);

    let rest = queue.drain();
    assert_eq!(rest.len(), 2);
    assert_eq!(rest[0].endpoint, "backup");
    assert_eq!(rest[1].endpoint, "observer");
    assert!(rest[1].body.report_only);
    assert!(queue.is_empty());
}

#[test]
fn endpoint_delivery_can_be_scheduled_without_reordering_other_endpoints() {
    let enforced = IntegrityPolicy::parse(
        "blocked-destinations=(style), endpoints=(security audit)",
    );
    let empty = IntegrityPolicy::default();
    let decision = evaluate_integrity_policy(
        &enforced,
        &empty,
        IntegrityPolicyDestination::Style,
        false,
        IntegrityPolicyRequestMode::NoCors,
        false,
    );

    let mut queue = IntegrityReportQueue::new();
    for path in ["a.css", "b.css", "c.css"] {
        queue.enqueue_decision(
            &Url::parse("https://app.test/").unwrap(),
            &Url::parse(&format!("https://cdn.test/{path}")).unwrap(),
            IntegrityPolicyDestination::Style,
            decision,
            &enforced,
            &empty,
        );
    }

    let security = queue.drain_endpoint("security");
    assert_eq!(security.len(), 3);
    assert!(security[0].body.blocked_url.ends_with("/a.css"));
    assert!(security[1].body.blocked_url.ends_with("/b.css"));
    assert!(security[2].body.blocked_url.ends_with("/c.css"));

    let remaining = queue.drain();
    assert_eq!(remaining.len(), 3);
    assert!(remaining.iter().all(|report| report.endpoint == "audit"));
    assert!(remaining[0].body.blocked_url.ends_with("/a.css"));
    assert!(remaining[2].body.blocked_url.ends_with("/c.css"));
}

#[test]
fn non_violations_do_not_create_reporting_work() {
    let mut queue = IntegrityReportQueue::new();
    let policy = IntegrityPolicy::default();
    let decision = evaluate_integrity_policy(
        &policy,
        &policy,
        IntegrityPolicyDestination::Script,
        true,
        IntegrityPolicyRequestMode::Cors,
        false,
    );

    assert_eq!(
        queue.enqueue_decision(
            &Url::parse("https://app.test/").unwrap(),
            &Url::parse("https://cdn.test/app.js").unwrap(),
            IntegrityPolicyDestination::Script,
            decision,
            &policy,
            &policy,
        ),
        0
    );
    assert!(queue.is_empty());
}
