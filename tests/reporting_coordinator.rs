use browser_engine::net::{ManualNetwork, NetworkBackend};
use browser_engine::{
    IntegrityViolationReport, IntegrityViolationReportBody, ReportingCoordinator,
    ReportingDeliveryDisposition, ReportingDeliveryOutcome, ReportingEndpoints,
    ReportingRetryDecision, ReportingRetryPolicy,
};

fn report(endpoint: &str, blocked: &str) -> IntegrityViolationReport {
    IntegrityViolationReport {
        report_type: "integrity-violation",
        endpoint: endpoint.into(),
        body: IntegrityViolationReportBody {
            document_url: "https://example.test/page".into(),
            blocked_url: blocked.into(),
            destination: "script".into(),
            report_only: false,
        },
    }
}

fn coordinator(mapping: &str) -> ReportingCoordinator {
    ReportingCoordinator::new(
        ReportingEndpoints::parse(mapping),
        ReportingRetryPolicy::new(100, 10_000, 3),
    )
}

#[test]
fn gone_delivery_is_applied_before_future_report_resolution() {
    let endpoint = "https://reports.test/collect";
    let network = ManualNetwork::new();
    network.respond_with(endpoint, 410, "text/plain", Vec::new());
    network.set_auto_complete(true);

    let mut coordinator = coordinator(r#"default="https://reports.test/collect""#);
    let reports = vec![report("default", "https://cdn.test/a.js")];
    let batches = coordinator.resolve_and_batch(&reports);
    assert_eq!(batches.len(), 1);
    let stale_batch = batches[0].clone();

    coordinator
        .queue_initial_batch(batches[0].clone(), 0, "toy-browser/1")
        .unwrap();
    assert_eq!(coordinator.dispatch(&network), 1);

    let (completed, unhandled, removed) =
        coordinator.process_completions(network.poll(), 1_000);
    assert!(unhandled.is_empty());
    assert_eq!(completed.len(), 1);
    assert_eq!(removed, 1);
    assert!(matches!(
        &completed[0].outcome,
        ReportingDeliveryOutcome::Delivered {
            disposition: ReportingDeliveryDisposition::RemoveEndpoint,
            ..
        }
    ));
    assert!(coordinator.endpoint_state().is_removed(&stale_batch.endpoint_url));
    assert!(coordinator.resolve_and_batch(&reports).is_empty());
    assert!(coordinator
        .queue_initial_batch(stale_batch, 0, "toy-browser/1")
        .is_err());
}

#[test]
fn ordinary_success_preserves_endpoint_for_future_reports() {
    let endpoint = "https://reports.test/collect";
    let network = ManualNetwork::new();
    network.respond_with(endpoint, 204, "text/plain", Vec::new());
    network.set_auto_complete(true);

    let mut coordinator = coordinator(r#"default="https://reports.test/collect""#);
    let reports = vec![report("default", "https://cdn.test/a.js")];
    let batches = coordinator.resolve_and_batch(&reports);
    coordinator
        .queue_initial_batch(batches[0].clone(), 25, "ua")
        .unwrap();
    coordinator.dispatch(&network);

    let (completed, unhandled, removed) =
        coordinator.process_completions(network.poll(), 500);
    assert!(unhandled.is_empty());
    assert_eq!(removed, 0);
    assert!(matches!(
        &completed[0].outcome,
        ReportingDeliveryOutcome::Delivered {
            disposition: ReportingDeliveryDisposition::Delivered,
            ..
        }
    ));
    assert_eq!(coordinator.endpoint_state().removed_len(), 0);
    assert_eq!(coordinator.resolve_and_batch(&reports).len(), 1);
}

#[test]
fn retryable_failure_keeps_endpoint_and_retry_state_in_one_coordinator() {
    let endpoint = "https://reports.test/collect";
    let network = ManualNetwork::new();
    network.respond_with(endpoint, 503, "text/plain", Vec::new());
    network.set_auto_complete(true);

    let mut coordinator = coordinator(r#"default="https://reports.test/collect""#);
    let reports = vec![report("default", "https://cdn.test/a.js")];
    let batches = coordinator.resolve_and_batch(&reports);
    coordinator
        .queue_initial_batch(batches[0].clone(), 50, "ua")
        .unwrap();
    coordinator.dispatch(&network);

    let (completed, unhandled, removed) =
        coordinator.process_completions(network.poll(), 1_000);
    assert!(unhandled.is_empty());
    assert_eq!(removed, 0);
    assert!(matches!(
        &completed[0].outcome,
        ReportingDeliveryOutcome::Retryable { .. }
    ));
    assert!(matches!(
        &completed[0].retry,
        Some(ReportingRetryDecision::Scheduled(_))
    ));
    assert_eq!(coordinator.retry_len(), 1);
    assert_eq!(coordinator.resolve_and_batch(&reports).len(), 1);

    assert!(coordinator
        .queue_ready_retries(1_099, 0, "ua")
        .is_empty());
    let queued = coordinator.queue_ready_retries(1_100, 0, "ua");
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].1, 2);
}

#[test]
fn committing_a_fresh_mapping_clears_prior_removal_state() {
    let endpoint = "https://reports.test/collect";
    let network = ManualNetwork::new();
    network.respond_with(endpoint, 410, "text/plain", Vec::new());
    network.set_auto_complete(true);

    let mapping = r#"default="https://reports.test/collect""#;
    let mut coordinator = coordinator(mapping);
    let reports = vec![report("default", "https://cdn.test/a.js")];
    let batch = coordinator.resolve_and_batch(&reports).remove(0);
    coordinator.queue_initial_batch(batch, 0, "ua").unwrap();
    coordinator.dispatch(&network);
    let (_, _, removed) = coordinator.process_completions(network.poll(), 10);
    assert_eq!(removed, 1);
    assert!(coordinator.resolve_and_batch(&reports).is_empty());

    coordinator.replace_endpoints(ReportingEndpoints::parse(mapping));
    assert_eq!(coordinator.endpoint_state().removed_len(), 0);
    assert_eq!(coordinator.resolve_and_batch(&reports).len(), 1);
}
