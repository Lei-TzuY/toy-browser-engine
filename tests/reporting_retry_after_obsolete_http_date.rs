use browser_engine::net::{FetchResponse, ManualNetwork, NetworkBackend};
use browser_engine::{
    IntegrityViolationReport, IntegrityViolationReportBody, ReportingDeliveryBatch,
    ReportingDeliveryRuntime, ReportingRetryPolicy, ResolvedIntegrityViolationReport, Url,
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

fn runtime_with_retry_after(value: &str) -> (ManualNetwork, ReportingDeliveryRuntime) {
    let network = ManualNetwork::new();
    let url = Url::parse("https://reports.test/collect").unwrap();
    let mut response = FetchResponse::synthetic(url, 503, Some("text/plain"), Vec::new());
    response.headers.insert_raw("retry-after", value);
    network.respond("https://reports.test/collect", response);
    network.set_auto_complete(true);

    let mut runtime = ReportingDeliveryRuntime::new(ReportingRetryPolicy::new(100, 10_000, 3));
    runtime
        .queue_initial(batch("https://reports.test/collect"), 250, "ua")
        .unwrap();
    runtime.dispatch(&network);
    (network, runtime)
}

#[test]
fn rfc850_retry_after_delays_reporting_redelivery() {
    let (network, mut runtime) = runtime_with_retry_after("Sunday, 06-Nov-94 08:49:40 GMT");

    // Sun, 06 Nov 1994 08:49:37 GMT in Unix milliseconds.
    runtime.process_completions_at(network.poll(), 1_000, 784_111_777_000);

    assert!(runtime.queue_ready_retries(3_999, 0, "ua").is_empty());
    assert_eq!(runtime.queue_ready_retries(4_000, 0, "ua").len(), 1);
}

#[test]
fn asctime_retry_after_delays_reporting_redelivery() {
    let (network, mut runtime) = runtime_with_retry_after("Sun Nov  6 08:49:40 1994");

    runtime.process_completions_at(network.poll(), 1_000, 784_111_777_000);

    assert!(runtime.queue_ready_retries(3_999, 0, "ua").is_empty());
    assert_eq!(runtime.queue_ready_retries(4_000, 0, "ua").len(), 1);
}
