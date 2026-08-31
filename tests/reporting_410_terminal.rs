use toy_browser_engine::integrity_policy_reporting::{
    IntegrityViolationReport, IntegrityViolationReportBody,
};
use toy_browser_engine::net::{ManualNetwork, NetworkBackend, Url};
use toy_browser_engine::reporting_delivery::ReportingDeliveryBatch;
use toy_browser_engine::reporting_endpoints::ResolvedIntegrityViolationReport;
use toy_browser_engine::reporting_retry::ReportingRetryPolicy;
use toy_browser_engine::reporting_runtime::ReportingDeliveryRuntime;
use toy_browser_engine::reporting_scheduler::ReportingDeliveryOutcome;

fn batch(endpoint: &str) -> ReportingDeliveryBatch {
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
                    blocked_url: "https://cdn.test/app.js".into(),
                    destination: "script".into(),
                    report_only: false,
                },
            },
        }],
    }
}

#[test]
fn gone_response_does_not_enter_retry_queue() {
    let network = ManualNetwork::new();
    network.respond_with(
        "https://reports.test/collect",
        410,
        "text/plain",
        Vec::new(),
    );
    network.set_auto_complete(true);

    let mut runtime =
        ReportingDeliveryRuntime::new(ReportingRetryPolicy::new(100, 10_000, 5));
    runtime
        .queue_initial(batch("https://reports.test/collect"), 0, "toy-browser/1")
        .unwrap();
    assert_eq!(runtime.dispatch(&network), 1);

    let (completed, unhandled) = runtime.process_completions(network.poll(), 1_000);
    assert!(unhandled.is_empty());
    assert_eq!(completed.len(), 1);
    assert!(matches!(
        completed[0].outcome,
        ReportingDeliveryOutcome::Delivered { .. }
    ));
    assert!(completed[0].retry.is_none());
    assert_eq!(runtime.retry_len(), 0);
    assert!(runtime.is_idle());
    assert!(runtime
        .queue_ready_retries(100_000, 100_000, "toy-browser/1")
        .is_empty());
    assert_eq!(network.requests().len(), 1);
}

#[test]
fn ordinary_failure_still_retries_but_410_does_not() {
    let network = ManualNetwork::new();
    network.respond_with(
        "https://reports.test/transient",
        503,
        "text/plain",
        Vec::new(),
    );
    network.respond_with(
        "https://reports.test/gone",
        410,
        "text/plain",
        Vec::new(),
    );
    network.set_auto_complete(true);

    let mut runtime =
        ReportingDeliveryRuntime::new(ReportingRetryPolicy::new(100, 10_000, 5));
    runtime
        .queue_initial(batch("https://reports.test/transient"), 0, "ua")
        .unwrap();
    runtime
        .queue_initial(batch("https://reports.test/gone"), 0, "ua")
        .unwrap();
    assert_eq!(runtime.dispatch(&network), 2);

    let (completed, unhandled) = runtime.process_completions(network.poll(), 5_000);
    assert!(unhandled.is_empty());
    assert_eq!(completed.len(), 2);
    assert_eq!(runtime.retry_len(), 1);

    let queued = runtime.queue_ready_retries(5_100, 0, "ua");
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].1, 2);
}
