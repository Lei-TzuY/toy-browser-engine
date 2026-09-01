//! Bounded retry scheduling for failed Reporting API deliveries.
//!
//! Reporting delivery is deliberately asynchronous. A transport timeout or a
//! non-2xx response should not turn into a tight retry loop, and a permanently
//! failing endpoint must not retain reports forever. This module keeps that
//! policy separate from the transport scheduler: failed batches are assigned a
//! bounded exponential delay and become eligible for a later network phase.

use crate::net::Url;
use crate::reporting_delivery::ReportingDeliveryBatch;
use crate::reporting_scheduler::ReportingDeliveryOutcome;

/// One second before the first retry.
pub const DEFAULT_REPORTING_RETRY_INITIAL_DELAY_MS: u64 = 1_000;
/// Cap exponential retry delay at one minute.
pub const DEFAULT_REPORTING_RETRY_MAX_DELAY_MS: u64 = 60_000;
/// Maximum delivery attempts, including the original attempt.
pub const DEFAULT_REPORTING_MAX_ATTEMPTS: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReportingRetryPolicy {
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub max_attempts: u32,
}

impl Default for ReportingRetryPolicy {
    fn default() -> Self {
        Self {
            initial_delay_ms: DEFAULT_REPORTING_RETRY_INITIAL_DELAY_MS,
            max_delay_ms: DEFAULT_REPORTING_RETRY_MAX_DELAY_MS,
            max_attempts: DEFAULT_REPORTING_MAX_ATTEMPTS,
        }
    }
}

impl ReportingRetryPolicy {
    pub fn new(initial_delay_ms: u64, max_delay_ms: u64, max_attempts: u32) -> Self {
        Self {
            initial_delay_ms,
            max_delay_ms: max_delay_ms.max(initial_delay_ms),
            max_attempts: max_attempts.max(1),
        }
    }

    /// Delay after `failed_attempt` (1 = the original attempt just failed).
    pub fn delay_after_failure(&self, failed_attempt: u32) -> u64 {
        let exponent = failed_attempt.saturating_sub(1).min(63);
        self.initial_delay_ms
            .saturating_mul(1u64 << exponent)
            .min(self.max_delay_ms)
    }

    /// Apply a server-requested minimum delay without allowing one response to
    /// retain a report beyond this queue's configured maximum backoff window.
    pub fn delay_after_failure_with_minimum(
        &self,
        failed_attempt: u32,
        minimum_delay_ms: u64,
    ) -> u64 {
        self.delay_after_failure(failed_attempt)
            .max(minimum_delay_ms)
            .min(self.max_delay_ms)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportingRetryEntry {
    pub batch: ReportingDeliveryBatch,
    /// Attempt number to use when this entry is delivered next.
    pub attempt: u32,
    pub ready_at_ms: u64,
    /// Lowest Reporting API `age` that may be serialized for this retry.
    ///
    /// This prevents callers from accidentally making a retried report appear
    /// younger than the preceding attempt when they provide a stale age value.
    pub minimum_age_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportingRetryDecision {
    Scheduled(ReportingRetryEntry),
    Dropped {
        batch: ReportingDeliveryBatch,
        attempts: u32,
    },
}

#[derive(Debug, Default)]
pub struct ReportingRetryQueue {
    policy: ReportingRetryPolicy,
    entries: Vec<ReportingRetryEntry>,
}

impl ReportingRetryQueue {
    pub fn new(policy: ReportingRetryPolicy) -> Self {
        Self {
            policy,
            entries: Vec::new(),
        }
    }

    pub fn policy(&self) -> ReportingRetryPolicy {
        self.policy
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Remove delayed retries targeting a concrete endpoint URL.
    ///
    /// A `410 Gone` removal applies to the endpoint itself, not merely to the
    /// delivery attempt that observed it. Any older failures for the same URL
    /// must therefore be discarded before they become eligible again.
    pub fn remove_endpoint(&mut self, endpoint_url: &Url) -> usize {
        let before = self.entries.len();
        self.entries
            .retain(|entry| &entry.batch.endpoint_url != endpoint_url);
        before - self.entries.len()
    }

    pub fn schedule_failure(
        &mut self,
        batch: ReportingDeliveryBatch,
        failed_attempt: u32,
        now_ms: u64,
    ) -> ReportingRetryDecision {
        self.schedule_failure_with_age(batch, failed_attempt, now_ms, 0)
    }

    /// Schedule a failed batch while carrying forward the report age used by
    /// the failed attempt. The retry delay is added to that age using saturating
    /// arithmetic so a later request cannot regress the Reporting API `age`.
    pub fn schedule_failure_with_age(
        &mut self,
        batch: ReportingDeliveryBatch,
        failed_attempt: u32,
        now_ms: u64,
        age_ms: u64,
    ) -> ReportingRetryDecision {
        self.schedule_failure_with_age_and_minimum_delay(
            batch,
            failed_attempt,
            now_ms,
            age_ms,
            0,
        )
    }

    /// Schedule a failed delivery while honoring a server-requested minimum
    /// delay (for example an HTTP `Retry-After` delta). The configured
    /// `max_delay_ms` remains a hard retention bound.
    pub fn schedule_failure_with_age_and_minimum_delay(
        &mut self,
        batch: ReportingDeliveryBatch,
        failed_attempt: u32,
        now_ms: u64,
        age_ms: u64,
        minimum_delay_ms: u64,
    ) -> ReportingRetryDecision {
        let failed_attempt = failed_attempt.max(1);
        if failed_attempt >= self.policy.max_attempts {
            return ReportingRetryDecision::Dropped {
                batch,
                attempts: failed_attempt,
            };
        }

        let delay_ms = self
            .policy
            .delay_after_failure_with_minimum(failed_attempt, minimum_delay_ms);
        let entry = ReportingRetryEntry {
            batch,
            attempt: failed_attempt.saturating_add(1),
            ready_at_ms: now_ms.saturating_add(delay_ms),
            minimum_age_ms: age_ms.saturating_add(delay_ms),
        };
        self.entries.push(entry.clone());
        ReportingRetryDecision::Scheduled(entry)
    }

    pub fn schedule_outcome(
        &mut self,
        outcome: ReportingDeliveryOutcome,
        attempt: u32,
        now_ms: u64,
    ) -> Option<ReportingRetryDecision> {
        self.schedule_outcome_with_age(outcome, attempt, now_ms, 0)
    }

    /// Consume a scheduler outcome with the age serialized for that attempt.
    pub fn schedule_outcome_with_age(
        &mut self,
        outcome: ReportingDeliveryOutcome,
        attempt: u32,
        now_ms: u64,
        age_ms: u64,
    ) -> Option<ReportingRetryDecision> {
        match outcome {
            ReportingDeliveryOutcome::Delivered { .. } => None,
            ReportingDeliveryOutcome::Retryable { batch, .. } => Some(
                self.schedule_failure_with_age(batch, attempt, now_ms, age_ms),
            ),
        }
    }

    pub fn drain_ready_up_to(&mut self, now_ms: u64, limit: usize) -> Vec<ReportingRetryEntry> {
        if limit == 0 {
            return Vec::new();
        }

        let mut ready = Vec::with_capacity(limit.min(self.entries.len()));
        let mut waiting = Vec::with_capacity(self.entries.len());
        for entry in self.entries.drain(..) {
            if entry.ready_at_ms <= now_ms && ready.len() < limit {
                ready.push(entry);
            } else {
                waiting.push(entry);
            }
        }
        self.entries = waiting;
        ready
    }

    pub fn drain_ready(&mut self, now_ms: u64) -> Vec<ReportingRetryEntry> {
        self.drain_ready_up_to(now_ms, usize::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrity_policy_reporting::{IntegrityViolationReport, IntegrityViolationReportBody};
    use crate::reporting_endpoints::ResolvedIntegrityViolationReport;

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
    fn exponential_delay_is_capped() {
        let policy = ReportingRetryPolicy::new(100, 450, 8);
        assert_eq!(policy.delay_after_failure(1), 100);
        assert_eq!(policy.delay_after_failure(2), 200);
        assert_eq!(policy.delay_after_failure(3), 400);
        assert_eq!(policy.delay_after_failure(4), 450);
        assert_eq!(policy.delay_after_failure(20), 450);
    }

    #[test]
    fn constructor_normalizes_invalid_limits() {
        let policy = ReportingRetryPolicy::new(500, 100, 0);
        assert_eq!(policy.initial_delay_ms, 500);
        assert_eq!(policy.max_delay_ms, 500);
        assert_eq!(policy.max_attempts, 1);
    }

    #[test]
    fn server_minimum_delay_extends_backoff_but_respects_cap() {
        let policy = ReportingRetryPolicy::new(100, 5_000, 4);
        assert_eq!(policy.delay_after_failure_with_minimum(1, 3_000), 3_000);
        assert_eq!(policy.delay_after_failure_with_minimum(2, 50), 200);
        assert_eq!(policy.delay_after_failure_with_minimum(2, 20_000), 5_000);
    }

    #[test]
    fn bounded_drain_preserves_ready_overflow() {
        let mut queue = ReportingRetryQueue::new(ReportingRetryPolicy::new(10, 10, 4));
        queue.schedule_failure(batch("https://reports.test/a"), 1, 0);
        queue.schedule_failure(batch("https://reports.test/b"), 1, 0);
        queue.schedule_failure(batch("https://reports.test/c"), 1, 0);

        let first = queue.drain_ready_up_to(10, 2);
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].batch.endpoint_url.to_string(), "https://reports.test/a");
        assert_eq!(first[1].batch.endpoint_url.to_string(), "https://reports.test/b");
        assert_eq!(queue.len(), 1);

        let second = queue.drain_ready_up_to(10, 2);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].batch.endpoint_url.to_string(), "https://reports.test/c");
        assert!(queue.is_empty());
    }

    #[test]
    fn removing_endpoint_discards_only_matching_delayed_retries() {
        let mut queue = ReportingRetryQueue::new(ReportingRetryPolicy::new(10, 10, 4));
        queue.schedule_failure(batch("https://reports.test/a"), 1, 0);
        queue.schedule_failure(batch("https://reports.test/b"), 1, 0);
        queue.schedule_failure(batch("https://reports.test/a"), 1, 0);

        let endpoint = Url::parse("https://reports.test/a").unwrap();
        assert_eq!(queue.remove_endpoint(&endpoint), 2);
        assert_eq!(queue.len(), 1);
        let ready = queue.drain_ready(10);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].batch.endpoint_url.to_string(), "https://reports.test/b");
        assert_eq!(queue.remove_endpoint(&endpoint), 0);
    }

    #[test]
    fn retry_minimum_age_includes_backoff_and_saturates() {
        let mut queue = ReportingRetryQueue::new(ReportingRetryPolicy::new(100, 100, 4));
        let decision = queue.schedule_failure_with_age(
            batch("https://reports.test/a"),
            1,
            u64::MAX - 50,
            u64::MAX - 25,
        );
        let ReportingRetryDecision::Scheduled(entry) = decision else {
            panic!("retry should be scheduled");
        };
        assert_eq!(entry.ready_at_ms, u64::MAX);
        assert_eq!(entry.minimum_age_ms, u64::MAX);
    }

    #[test]
    fn server_minimum_delay_advances_ready_time_and_age_together() {
        let mut queue = ReportingRetryQueue::new(ReportingRetryPolicy::new(100, 10_000, 4));
        let decision = queue.schedule_failure_with_age_and_minimum_delay(
            batch("https://reports.test/a"),
            1,
            1_000,
            250,
            3_000,
        );
        let ReportingRetryDecision::Scheduled(entry) = decision else {
            panic!("retry should be scheduled");
        };
        assert_eq!(entry.ready_at_ms, 4_000);
        assert_eq!(entry.minimum_age_ms, 3_250);
    }
}
