use browser_engine::integrity_policy_reporting::{
    IntegrityViolationReport, IntegrityViolationReportBody,
};
use browser_engine::net::{FetchError, ManualNetwork, NetworkBackend, Url};
use browser_engine::reporting_delivery::ReportingDeliveryBatch;
use browser_engine::reporting_endpoints::ResolvedIntegrityViolationReport;
use browser_engine::{
    ReportingDeliveryFailure, ReportingDeliveryOutcome, ReportingDeliveryScheduler,
};

fn batch(endpoint: &str, blocked_url: &str) -> ReportingDeliveryBatch {
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
                    blocked_url: blocked_url.into(),
                    destination: "script".into(),
                    report_only: false,
                },
            },
        }],
    }
}

#[test]
fn scheduler_dispatches_browser_owned_reporting_posts_in_fifo_order() {
    let network = ManualNetwork::new();
    let mut scheduler = ReportingDeliveryScheduler::new();
    let first = scheduler
        .queue(
            batch("https://reports.test/first", "https://cdn.test/a.js"),
            25,
            "toy-browser/1.0",
        )
        .unwrap();
    let second = scheduler
        .queue(
            batch("https://reports.test/second", "https://cdn.test/b.js"),
            25,
            "toy-browser/1.0",
        )
        .unwrap();

    assert!(second > first);
    assert_eq!(scheduler.dispatch(&network), 2);
    let requests = network.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].url.to_string(), "https://reports.test/first");
    assert_eq!(requests[1].url.to_string(), "https://reports.test/second");
    assert_eq!(requests[0].method.as_str(), "POST");
    assert_eq!(
        requests[0].headers.get("content-type").as_deref(),
        Some("application/reports+json")
    );
    assert!(!requests[0].headers.has("cookie"));
    assert!(!requests[0].headers.has("authorization"));
}

#[test]
fn scheduler_surfaces_http_and_network_failures_with_original_batches() {
    let network = ManualNetwork::new();
    network.respond_with("https://reports.test/http", 429, "text/plain", Vec::new());
    network.fail(
        "https://reports.test/network",
        FetchError::Timeout("collector".into()),
    );
    network.set_auto_complete(true);

    let mut scheduler = ReportingDeliveryScheduler::new();
    scheduler
        .queue(
            batch("https://reports.test/http", "https://cdn.test/a.js"),
            0,
            "ua",
        )
        .unwrap();
    scheduler
        .queue(
            batch("https://reports.test/network", "https://cdn.test/b.js"),
            0,
            "ua",
        )
        .unwrap();
    scheduler.dispatch(&network);

    let (outcomes, unhandled) = scheduler.process_completions(network.poll());
    assert!(unhandled.is_empty());
    assert_eq!(outcomes.len(), 2);
    match &outcomes[0] {
        ReportingDeliveryOutcome::Retryable { batch, failure, .. } => {
            assert_eq!(batch.endpoint_url.to_string(), "https://reports.test/http");
            assert_eq!(failure, &ReportingDeliveryFailure::HttpStatus(429));
        }
        other => panic!("expected retryable HTTP failure, got {other:?}"),
    }
    match &outcomes[1] {
        ReportingDeliveryOutcome::Retryable { batch, failure, .. } => {
            assert_eq!(batch.endpoint_url.to_string(), "https://reports.test/network");
            assert!(matches!(
                failure,
                ReportingDeliveryFailure::Network(FetchError::Timeout(_))
            ));
        }
        other => panic!("expected retryable network failure, got {other:?}"),
    }
    assert!(scheduler.is_empty());
}

#[test]
fn successful_delivery_is_consumed_without_retry_work() {
    let network = ManualNetwork::new();
    network.respond_with("https://reports.test/ok", 202, "text/plain", Vec::new());
    network.set_auto_complete(true);

    let mut scheduler = ReportingDeliveryScheduler::new();
    scheduler
        .queue(
            batch("https://reports.test/ok", "https://cdn.test/app.js"),
            3,
            "ua",
        )
        .unwrap();
    scheduler.dispatch(&network);
    let (outcomes, unhandled) = scheduler.process_completions(network.poll());

    assert!(unhandled.is_empty());
    assert!(matches!(
        outcomes.as_slice(),
        [ReportingDeliveryOutcome::Delivered { .. }]
    ));
    assert!(scheduler.is_empty());
}
