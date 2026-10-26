use browser_engine::integrity_policy_reporting::{
    IntegrityViolationReport, IntegrityViolationReportBody,
};
use browser_engine::net::{FetchResponse, ManualNetwork, NetworkBackend, Url};
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

fn response_with_retry_after(url: &str, status: u16, value: &str) -> FetchResponse {
    let mut response = FetchResponse::synthetic(
        Url::parse(url).unwrap(),
        status,
        Some("text/plain"),
        Vec::new(),
    );
    response.headers.insert_raw("retry-after", value);
    response
}

#[test]
fn retry_after_delta_seconds_delays_redelivery_and_advances_age() {
    let endpoint = "https://reports.test/collect";
    let network = ManualNetwork::new();
    network.respond(endpoint, response_with_retry_after(endpoint, 503, "4"));
    network.set_auto_complete(true);

    let mut runtime = ReportingDeliveryRuntime::new(ReportingRetryPolicy::new(100, 10_000, 3));
    runtime.queue_initial(batch(endpoint), 200, "toy/1").unwrap();
    assert_eq!(runtime.dispatch(&network), 1);
    runtime.process_completions(network.poll(), 1_000);

    assert!(runtime.queue_ready_retries(4_999, 0, "toy/1").is_empty());
    assert_eq!(runtime.queue_ready_retries(5_000, 0, "toy/1").len(), 1);
    runtime.dispatch(&network);

    let requests = network.requests();
    assert_eq!(requests.len(), 2);
    let body = String::from_utf8(requests[1].body.clone().unwrap()).unwrap();
    assert!(body.contains("\"age\":4200"), "{body}");
}

#[test]
fn malformed_retry_after_falls_back_to_local_backoff() {
    let endpoint = "https://reports.test/collect";
    let network = ManualNetwork::new();
    network.respond(
        endpoint,
        response_with_retry_after(endpoint, 429, "soon-ish"),
    );
    network.set_auto_complete(true);

    let mut runtime = ReportingDeliveryRuntime::new(ReportingRetryPolicy::new(250, 10_000, 3));
    runtime.queue_initial(batch(endpoint), 0, "toy/1").unwrap();
    runtime.dispatch(&network);
    runtime.process_completions(network.poll(), 2_000);

    assert!(runtime.queue_ready_retries(2_249, 0, "toy/1").is_empty());
    assert_eq!(runtime.queue_ready_retries(2_250, 0, "toy/1").len(), 1);
}

#[test]
fn retry_after_cannot_exceed_configured_retention_cap() {
    let endpoint = "https://reports.test/collect";
    let network = ManualNetwork::new();
    network.respond(endpoint, response_with_retry_after(endpoint, 503, "3600"));
    network.set_auto_complete(true);

    let mut runtime = ReportingDeliveryRuntime::new(ReportingRetryPolicy::new(100, 5_000, 3));
    runtime.queue_initial(batch(endpoint), 50, "toy/1").unwrap();
    runtime.dispatch(&network);
    runtime.process_completions(network.poll(), 10_000);

    assert!(runtime.queue_ready_retries(14_999, 0, "toy/1").is_empty());
    assert_eq!(runtime.queue_ready_retries(15_000, 0, "toy/1").len(), 1);
}
