use browser_engine::integrity_policy_reporting::{
    IntegrityViolationReport, IntegrityViolationReportBody,
};
use browser_engine::net::{ManualNetwork, Url};
use browser_engine::{
    ReportingDeliveryBatch, ReportingDeliveryRuntime, ReportingRetryPolicy,
    ResolvedIntegrityViolationReport,
};

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
fn retry_never_serializes_an_age_younger_than_prior_age_plus_backoff() {
    let network = ManualNetwork::new();
    network.respond_with("https://reports.test/collect", 503, "text/plain", Vec::new());
    network.set_auto_complete(true);

    let mut runtime = ReportingDeliveryRuntime::new(ReportingRetryPolicy::new(100, 100, 3));
    runtime
        .queue_initial(batch("https://reports.test/collect"), 250, "toy/1")
        .unwrap();
    runtime.dispatch(&network);
    runtime.process_completions(network.poll(), 1_000);

    // Deliberately pass a stale age. Runtime must carry the previous 250ms age
    // forward and include at least the 100ms retry delay.
    let queued = runtime.queue_ready_retries(1_100, 10, "toy/1");
    assert_eq!(queued.len(), 1);
    runtime.dispatch(&network);

    let requests = network.requests();
    assert_eq!(requests.len(), 2);
    let retry_body = String::from_utf8(requests[1].body.clone().unwrap()).unwrap();
    assert!(retry_body.contains("\"age\":350"), "{retry_body}");
}

#[test]
fn fresher_caller_age_wins_over_retry_minimum() {
    let network = ManualNetwork::new();
    network.respond_with("https://reports.test/collect", 503, "text/plain", Vec::new());
    network.set_auto_complete(true);

    let mut runtime = ReportingDeliveryRuntime::new(ReportingRetryPolicy::new(100, 100, 3));
    runtime
        .queue_initial(batch("https://reports.test/collect"), 50, "toy/1")
        .unwrap();
    runtime.dispatch(&network);
    runtime.process_completions(network.poll(), 500);

    runtime.queue_ready_retries(600, 900, "toy/1");
    runtime.dispatch(&network);
    let requests = network.requests();
    let retry_body = String::from_utf8(requests[1].body.clone().unwrap()).unwrap();
    assert!(retry_body.contains("\"age\":900"), "{retry_body}");
}
