use browser_engine::net::{FetchCompletion, FetchResponse};
use browser_engine::{
    IntegrityViolationReport, IntegrityViolationReportBody, ReportingCoordinator,
    ReportingEndpoints, ReportingRetryPolicy, Url,
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

fn response(id: u64, endpoint: &str, status: u16) -> FetchCompletion {
    FetchCompletion {
        id,
        result: Ok(FetchResponse::synthetic(
            Url::parse(endpoint).unwrap(),
            status,
            Some("text/plain"),
            Vec::new(),
        )),
    }
}

#[test]
fn in_flight_failure_after_410_does_not_recreate_retry() {
    let endpoint = "https://reports.test/collect";
    let endpoint_url = Url::parse(endpoint).unwrap();
    let mut coordinator = ReportingCoordinator::new(
        ReportingEndpoints::parse(r#"default="https://reports.test/collect""#),
        ReportingRetryPolicy::new(100, 1_000, 4),
    );
    let reports = vec![report("default", "https://cdn.test/a.js")];
    let batch = coordinator.resolve_and_batch(&reports).remove(0);

    let gone_id = coordinator
        .queue_initial_batch(batch.clone(), 0, "ua")
        .unwrap();
    let still_in_flight_id = coordinator.queue_initial_batch(batch, 0, "ua").unwrap();

    let (completed, unhandled, removed) =
        coordinator.process_completions(vec![response(gone_id, endpoint, 410)], 1_000);
    assert_eq!(completed.len(), 1);
    assert!(unhandled.is_empty());
    assert_eq!(removed, 1);
    assert!(coordinator.endpoint_state().is_removed(&endpoint_url));
    assert_eq!(coordinator.in_flight_len(), 1);
    assert_eq!(coordinator.retry_len(), 0);

    let (completed, unhandled, removed) = coordinator.process_completions(
        vec![response(still_in_flight_id, endpoint, 503)],
        1_010,
    );
    assert_eq!(completed.len(), 1);
    assert!(unhandled.is_empty());
    assert_eq!(removed, 0);
    assert_eq!(coordinator.in_flight_len(), 0);
    assert_eq!(coordinator.retry_len(), 0);
    assert!(coordinator.queue_ready_retries(10_000, 0, "ua").is_empty());
    assert!(coordinator.is_idle());
}

#[test]
fn late_failure_for_other_endpoint_still_retries_after_primary_410() {
    let primary = "https://reports.test/primary";
    let backup = "https://reports.test/backup";
    let mut coordinator = ReportingCoordinator::new(
        ReportingEndpoints::parse(
            r#"primary="https://reports.test/primary", backup="https://reports.test/backup""#,
        ),
        ReportingRetryPolicy::new(100, 1_000, 4),
    );

    let primary_batch = coordinator
        .resolve_and_batch(&[report("primary", "https://cdn.test/a.js")])
        .remove(0);
    let backup_batch = coordinator
        .resolve_and_batch(&[report("backup", "https://cdn.test/b.js")])
        .remove(0);

    let gone_id = coordinator
        .queue_initial_batch(primary_batch, 0, "ua")
        .unwrap();
    let backup_id = coordinator
        .queue_initial_batch(backup_batch, 0, "ua")
        .unwrap();

    let (_, unhandled, removed) =
        coordinator.process_completions(vec![response(gone_id, primary, 410)], 1_000);
    assert!(unhandled.is_empty());
    assert_eq!(removed, 1);

    let (_, unhandled, removed) =
        coordinator.process_completions(vec![response(backup_id, backup, 503)], 1_010);
    assert!(unhandled.is_empty());
    assert_eq!(removed, 0);
    assert_eq!(coordinator.retry_len(), 1);
    assert!(coordinator.queue_ready_retries(1_109, 0, "ua").is_empty());
    let ready = coordinator.queue_ready_retries(1_110, 0, "ua");
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].1, 2);
}
