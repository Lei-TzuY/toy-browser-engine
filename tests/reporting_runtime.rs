use browser_engine::integrity_policy_reporting::{
    IntegrityViolationReport, IntegrityViolationReportBody,
};
use browser_engine::net::{
    FetchRequest, HeaderMap, ManualNetwork, Method, NetworkBackend, Url,
};
use browser_engine::{
    ReportingDeliveryBatch, ReportingDeliveryFailure, ReportingDeliveryOutcome,
    ReportingDeliveryRuntime, ReportingRetryDecision, ReportingRetryPolicy,
    ResolvedIntegrityViolationReport,
};

fn batch(endpoint: &str, blocked: &str) -> ReportingDeliveryBatch {
    let endpoint_url = Url::parse(endpoint).unwrap();
    ReportingDeliveryBatch {
        endpoint_url: endpoint_url.clone(),
        reports: vec![ResolvedIntegrityViolationReport {
            endpoint_name: "default".into(),
            endpoint_url,
            report: IntegrityViolationReport {
                report_type: "integrity-violation",
                endpoint: "default".into(),
                body: IntegrityViolationReportBody {
                    document_url: "https://example.test/page".into(),
                    blocked_url: blocked.into(),
                    destination: "script".into(),
                    report_only: false,
                },
            },
        }],
    }
}

#[test]
fn retryable_http_failure_is_redelivered_after_backoff() {
    let network = ManualNetwork::new();
    network.respond_with(
        "https://reports.test/collect",
        503,
        "text/plain",
        Vec::new(),
    );
    network.set_auto_complete(true);

    let mut runtime =
        ReportingDeliveryRuntime::new(ReportingRetryPolicy::new(50, 1_000, 3));
    runtime
        .queue_initial(
            batch("https://reports.test/collect", "https://cdn.test/a.js"),
            0,
            "toy/1",
        )
        .unwrap();
    assert_eq!(runtime.dispatch(&network), 1);

    let (first, page) = runtime.process_completions(network.poll(), 1_000);
    assert!(page.is_empty());
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].attempt, 1);
    assert!(matches!(
        first[0].outcome,
        ReportingDeliveryOutcome::Retryable {
            failure: ReportingDeliveryFailure::HttpStatus(503),
            ..
        }
    ));
    assert!(matches!(
        first[0].retry,
        Some(ReportingRetryDecision::Scheduled(_))
    ));

    assert!(runtime.queue_ready_retries(1_049, 49, "toy/1").is_empty());
    let retry = runtime.queue_ready_retries(1_050, 50, "toy/1");
    assert_eq!(retry.len(), 1);
    assert_eq!(retry[0].1, 2);

    network.respond_with(
        "https://reports.test/collect",
        204,
        "text/plain",
        Vec::new(),
    );
    assert_eq!(runtime.dispatch(&network), 1);
    let (second, page) = runtime.process_completions(network.poll(), 1_050);
    assert!(page.is_empty());
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].attempt, 2);
    assert!(matches!(
        second[0].outcome,
        ReportingDeliveryOutcome::Delivered { .. }
    ));
    assert!(second[0].retry.is_none());
    assert!(runtime.is_idle());
}

#[test]
fn scheduler_capacity_does_not_drop_ready_retries() {
    let network = ManualNetwork::new();
    network.respond_with("https://reports.test/a", 503, "text/plain", Vec::new());
    network.respond_with("https://reports.test/b", 503, "text/plain", Vec::new());
    network.set_auto_complete(true);

    let mut runtime = ReportingDeliveryRuntime::new(ReportingRetryPolicy::new(10, 10, 4))
        .with_in_flight_limit(1);

    runtime
        .queue_initial(
            batch("https://reports.test/a", "https://cdn.test/a.js"),
            0,
            "ua",
        )
        .unwrap();
    runtime.dispatch(&network);
    runtime.process_completions(network.poll(), 0);

    runtime
        .queue_initial(
            batch("https://reports.test/b", "https://cdn.test/b.js"),
            0,
            "ua",
        )
        .unwrap();
    runtime.dispatch(&network);
    runtime.process_completions(network.poll(), 0);
    assert_eq!(runtime.retry_len(), 2);

    let first = runtime.queue_ready_retries(10, 10, "ua");
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].1, 2);
    assert_eq!(runtime.retry_len(), 1);
    assert!(runtime.queue_ready_retries(10, 10, "ua").is_empty());
    assert_eq!(runtime.retry_len(), 1);

    runtime.dispatch(&network);
    runtime.process_completions(network.poll(), 10);
    let second = runtime.queue_ready_retries(10, 10, "ua");
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].1, 2);
}

#[test]
fn unrelated_page_fetch_completion_is_returned_untouched() {
    let network = ManualNetwork::new();
    network.respond_with(
        "https://example.test/data",
        200,
        "text/plain",
        b"page".to_vec(),
    );
    network.set_auto_complete(true);
    network.start(
        7,
        FetchRequest::new(
            Url::parse("https://example.test/data").unwrap(),
            Method::Get,
            HeaderMap::new(),
            None,
        ),
    );

    let mut runtime = ReportingDeliveryRuntime::new(ReportingRetryPolicy::default());
    let (completed, page) = runtime.process_completions(network.poll(), 0);
    assert!(completed.is_empty());
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].id, 7);
}

#[test]
fn exhausted_attempt_is_dropped_instead_of_requeued() {
    let network = ManualNetwork::new();
    network.respond_with(
        "https://reports.test/drop",
        500,
        "text/plain",
        Vec::new(),
    );
    network.set_auto_complete(true);

    let mut runtime =
        ReportingDeliveryRuntime::new(ReportingRetryPolicy::new(1, 1, 1));
    runtime
        .queue_initial(
            batch("https://reports.test/drop", "https://cdn.test/drop.js"),
            0,
            "ua",
        )
        .unwrap();
    runtime.dispatch(&network);
    let (completed, page) = runtime.process_completions(network.poll(), 10);
    assert!(page.is_empty());
    assert!(matches!(
        completed[0].retry,
        Some(ReportingRetryDecision::Dropped { attempts: 1, .. })
    ));
    assert!(runtime.is_idle());
}
