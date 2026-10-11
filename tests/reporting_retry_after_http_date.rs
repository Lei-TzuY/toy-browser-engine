use browser_engine::net::{FetchResponse, ManualNetwork, NetworkBackend};
use browser_engine::{
    ReportingDeliveryBatch, ReportingDeliveryRuntime, ReportingRetryPolicy,
    ResolvedIntegrityViolationReport, IntegrityViolationReport, IntegrityViolationReportBody, Url,
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
fn http_date_retry_after_delays_reporting_redelivery() {
    let network = ManualNetwork::new();
    let url = Url::parse("https://reports.test/collect").unwrap();
    let mut response = FetchResponse::synthetic(url, 503, Some("text/plain"), Vec::new());
    response
        .headers
        .insert_raw("retry-after", "Sun, 06 Nov 1994 08:49:40 GMT");
    network.respond("https://reports.test/collect", response);
    network.set_auto_complete(true);

    let mut runtime = ReportingDeliveryRuntime::new(ReportingRetryPolicy::new(100, 10_000, 3));
    runtime
        .queue_initial(batch("https://reports.test/collect"), 250, "ua")
        .unwrap();
    runtime.dispatch(&network);

    // Sun, 06 Nov 1994 08:49:37 GMT in Unix milliseconds.
    let wall_now = 784_111_777_000;
    runtime.process_completions_at(network.poll(), 1_000, wall_now);

    assert!(runtime.queue_ready_retries(3_999, 0, "ua").is_empty());
    assert_eq!(runtime.queue_ready_retries(4_000, 0, "ua").len(), 1);
}

#[test]
fn past_http_date_falls_back_to_local_backoff_floor() {
    let network = ManualNetwork::new();
    let url = Url::parse("https://reports.test/collect").unwrap();
    let mut response = FetchResponse::synthetic(url, 503, Some("text/plain"), Vec::new());
    response
        .headers
        .insert_raw("retry-after", "Sun, 06 Nov 1994 08:49:30 GMT");
    network.respond("https://reports.test/collect", response);
    network.set_auto_complete(true);

    let mut runtime = ReportingDeliveryRuntime::new(ReportingRetryPolicy::new(250, 10_000, 3));
    runtime
        .queue_initial(batch("https://reports.test/collect"), 0, "ua")
        .unwrap();
    runtime.dispatch(&network);

    runtime.process_completions_at(network.poll(), 5_000, 784_111_777_000);
    assert!(runtime.queue_ready_retries(5_249, 0, "ua").is_empty());
    assert_eq!(runtime.queue_ready_retries(5_250, 0, "ua").len(), 1);
}
