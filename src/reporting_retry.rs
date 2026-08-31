//! Bounded retry scheduling for failed Reporting API deliveries.
//!
//! Reporting delivery is deliberately asynchronous. A transport timeout or a
//! non-2xx response should not turn into a tight retry loop, and a permanently
//! failing endpoint must not retain reports forever. This module keeps that
//! policy separate from the transport scheduler: failed batches are assigned a
//! bounded exponential delay and become eligible for a later network phase.

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportingRetryEntry {
    pub batch: ReportingDeliveryBatch,
    /// Attempt number to use when this entry is delivered next.
    pub attempt: u32,
    pub ready_at_ms: u64,
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

    /// Schedule a batch after a failed delivery attempt.
    ///
    /// `failed_attempt` starts at 1 for the original delivery. Once the maximum
    /// number of attempts has been consumed the batch is returned as dropped
    /// instead of being retained forever.
    pub fn schedule_failure(
        &mut self,
        batch: ReportingDeliveryBatch,
        failed_attempt: u32,
        now_ms: u64,
    ) -> ReportingRetryDecision {
        let failed_attempt = failed_attempt.max(1);
        if failed_attempt >= self.policy.max_attempts {
            return ReportingRetryDecision::Dropped {
                batch,
                attempts: failed_attempt,
            };
        }

        let entry = ReportingRetryEntry {
            batch,
            attempt: failed_attempt.saturating_add(1),
            ready_at_ms: now_ms.saturating_add(self.policy.delay_after_failure(failed_attempt)),
        };
        self.entries.push(entry.clone());
        ReportingRetryDecision::Scheduled(entry)
    }

    /// Consume a scheduler outcome when it represents a retryable failure.
    /// Successful deliveries are intentionally ignored.
    pub fn schedule_outcome(
        &mut self,
        outcome: ReportingDeliveryOutcome,
        attempt: u32,
        now_ms: u64,
    ) -> Option<ReportingRetryDecision> {
        match outcome {
            ReportingDeliveryOutcome::Delivered { .. } => None,
            ReportingDeliveryOutcome::Retryable { batch, .. } => {
                Some(self.schedule_failure(batch, attempt, now_ms))
            }
        }
    }

    /// Remove and return all entries whose delay has elapsed.
    ///
    /// Insertion order is preserved both among ready entries and among entries
    /// that remain queued, keeping retry dispatch deterministic.
    pub fn drain_ready(&mut self, now_ms: u64) -> Vec<ReportingRetryEntry> {
        let mut ready = Vec::new();
        let mut waiting = Vec::with_capacity(self.entries.len());
        for entry in self.entries.drain(..) {
            if entry.ready_at_ms <= now_ms {
                ready.push(entry);
            } else {
                waiting.push(entry);
            }
        }
        self.entries = waiting;
        ready
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
