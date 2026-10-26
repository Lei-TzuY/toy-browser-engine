use browser_engine::reporting_scheduler::ReportingDeliveryDisposition;
use browser_engine::{
    IntegrityViolationReport, IntegrityViolationReportBody, ReportingDeliveryBatch,
    ReportingDeliveryFailure, ReportingDeliveryOutcome, ReportingRetryDecision,
    ReportingRetryPolicy, ReportingRetryQueue, ResolvedIntegrityViolationReport, Url,
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
fn retryable_scheduler_outcome_is_delayed_then_released() {
    let original = batch("https://reports.test/collect", "https://cdn.test/a.js");
    let outcome = ReportingDeliveryOutcome::Retryable {
        id: 42,
        batch: original.clone(),
        failure: ReportingDeliveryFailure::HttpStatus(503),
    };
    let mut retries = ReportingRetryQueue::new(ReportingRetryPolicy::new(1_000, 8_000, 5));

    let decision = retries.schedule_outcome(outcome, 1, 10_000).unwrap();
    assert!(matches!(
        decision,
        ReportingRetryDecision::Scheduled(ref entry)
            if entry.attempt == 2 && entry.ready_at_ms == 11_000 && entry.batch == original
    ));
    assert!(retries.drain_ready(10_999).is_empty());
    let ready = retries.drain_ready(11_000);
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].attempt, 2);
    assert_eq!(ready[0].batch, original);
    assert!(retries.is_empty());
}

#[test]
fn repeated_failures_back_off_and_stop_at_attempt_limit() {
    let original = batch("https://reports.test/collect", "https://cdn.test/a.js");
    let mut retries = ReportingRetryQueue::new(ReportingRetryPolicy::new(100, 250, 3));

    let first = retries.schedule_failure(original.clone(), 1, 1_000);
    assert!(matches!(
        first,
        ReportingRetryDecision::Scheduled(ref entry)
            if entry.attempt == 2 && entry.ready_at_ms == 1_100
    ));
    let second_batch = retries.drain_ready(1_100).remove(0).batch;

    let second = retries.schedule_failure(second_batch, 2, 1_100);
    assert!(matches!(
        second,
        ReportingRetryDecision::Scheduled(ref entry)
            if entry.attempt == 3 && entry.ready_at_ms == 1_300
    ));
    let third_batch = retries.drain_ready(1_300).remove(0).batch;

    let final_decision = retries.schedule_failure(third_batch, 3, 1_300);
    assert!(matches!(
        final_decision,
        ReportingRetryDecision::Dropped { attempts: 3, ref batch } if *batch == original
    ));
    assert!(retries.is_empty());
}

#[test]
fn ready_drain_preserves_fifo_and_leaves_later_entries_queued() {
    let mut retries = ReportingRetryQueue::new(ReportingRetryPolicy::new(100, 1_000, 5));
    let a = batch("https://reports.test/a", "https://cdn.test/a.js");
    let b = batch("https://reports.test/b", "https://cdn.test/b.js");
    let c = batch("https://reports.test/c", "https://cdn.test/c.js");

    retries.schedule_failure(a.clone(), 1, 1_000); // ready 1100
    retries.schedule_failure(b.clone(), 2, 1_000); // ready 1200
    retries.schedule_failure(c.clone(), 1, 1_050); // ready 1150

    let first = retries.drain_ready(1_150);
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].batch, a);
    assert_eq!(first[1].batch, c);
    assert_eq!(retries.len(), 1);

    let second = retries.drain_ready(1_200);
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].batch, b);
}

#[test]
fn delivered_outcome_never_enters_retry_queue() {
    let mut retries = ReportingRetryQueue::default();
    let outcome = ReportingDeliveryOutcome::Delivered {
        id: 7,
        batch: batch("https://reports.test/collect", "https://cdn.test/app.js"),
        disposition: ReportingDeliveryDisposition::Delivered,
    };
    assert_eq!(retries.schedule_outcome(outcome, 1, 0), None);
    assert!(retries.is_empty());
}

#[test]
fn timestamp_math_saturates_instead_of_wrapping() {
    let original = batch("https://reports.test/collect", "https://cdn.test/a.js");
    let mut retries = ReportingRetryQueue::new(ReportingRetryPolicy::new(10, 10, 5));
    let decision = retries.schedule_failure(original, 1, u64::MAX - 2);
    assert!(matches!(
        decision,
        ReportingRetryDecision::Scheduled(ref entry) if entry.ready_at_ms == u64::MAX
    ));
}
