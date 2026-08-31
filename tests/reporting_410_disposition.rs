use browser_engine::integrity_policy_reporting::{
    IntegrityViolationReport, IntegrityViolationReportBody,
};
use browser_engine::net::{ManualNetwork, NetworkBackend, Url};
use browser_engine::reporting_scheduler::ReportingDeliveryDisposition;
use browser_engine::{
    ReportingDeliveryBatch, ReportingDeliveryOutcome, ReportingDeliveryRuntime,
    ReportingRetryPolicy, ResolvedIntegrityViolationReport,
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
fn successful_delivery_and_endpoint_removal_are_observably_distinct() {
    let network = ManualNetwork::new();
    network.respond_with(
        "https://reports.test/success",
        204,
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
        .queue_initial(batch("https://reports.test/success"), 0, "ua")
        .unwrap();
    runtime
        .queue_initial(batch("https://reports.test/gone"), 0, "ua")
        .unwrap();
    assert_eq!(runtime.dispatch(&network), 2);

    let (completed, unhandled) = runtime.process_completions(network.poll(), 1_000);
    assert!(unhandled.is_empty());
    assert_eq!(completed.len(), 2);

    assert!(matches!(
        completed[0].outcome,
        ReportingDeliveryOutcome::Delivered {
            disposition: ReportingDeliveryDisposition::Delivered,
            ..
        }
    ));
    assert!(matches!(
        completed[1].outcome,
        ReportingDeliveryOutcome::Delivered {
            disposition: ReportingDeliveryDisposition::RemoveEndpoint,
            ..
        }
    ));
    assert!(completed.iter().all(|completion| completion.retry.is_none()));
    assert!(runtime.is_idle());
}
