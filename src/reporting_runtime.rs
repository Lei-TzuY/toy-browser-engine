//! End-to-end runtime coordination for Reporting API delivery and retries.
//!
//! The transport scheduler and retry queue intentionally have separate jobs:
//! one owns browser-generated network requests, while the other owns delayed
//! retry eligibility. This coordinator closes the loop without letting either
//! component steal responsibilities from the browser event loop.

use std::collections::HashMap;

use crate::net::{FetchCompletion, FetchError, FetchId, NetworkBackend};
use crate::reporting_delivery::ReportingDeliveryBatch;
use crate::reporting_retry::{
    ReportingRetryDecision, ReportingRetryPolicy, ReportingRetryQueue,
};
use crate::reporting_scheduler::{ReportingDeliveryOutcome, ReportingDeliveryScheduler};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportingRuntimeCompletion {
    pub attempt: u32,
    pub outcome: ReportingDeliveryOutcome,
    pub retry: Option<ReportingRetryDecision>,
}

#[derive(Debug, Clone, Copy)]
struct ReportingAttemptState {
    attempt: u32,
    age_ms: u64,
}

/// Owns the live Reporting API scheduler plus delayed retry state.
#[derive(Debug)]
pub struct ReportingDeliveryRuntime {
    scheduler: ReportingDeliveryScheduler,
    retries: ReportingRetryQueue,
    attempts: HashMap<FetchId, ReportingAttemptState>,
}

impl ReportingDeliveryRuntime {
    pub fn new(policy: ReportingRetryPolicy) -> Self {
        Self {
            scheduler: ReportingDeliveryScheduler::new(),
            retries: ReportingRetryQueue::new(policy),
            attempts: HashMap::new(),
        }
    }

    pub fn with_in_flight_limit(mut self, limit: usize) -> Self {
        self.scheduler = self.scheduler.with_limit(limit);
        self
    }

    pub fn in_flight_len(&self) -> usize {
        self.scheduler.len()
    }

    pub fn retry_len(&self) -> usize {
        self.retries.len()
    }

    pub fn is_idle(&self) -> bool {
        self.scheduler.is_empty() && self.retries.is_empty()
    }

    /// Queue a newly-created delivery batch as attempt 1.
    pub fn queue_initial(
        &mut self,
        batch: ReportingDeliveryBatch,
        age_ms: u64,
        user_agent: &str,
    ) -> Result<FetchId, FetchError> {
        let id = self.scheduler.queue(batch, age_ms, user_agent)?;
        self.attempts.insert(id, ReportingAttemptState { attempt: 1, age_ms });
        Ok(id)
    }

    /// Move eligible retries back into the transport scheduler without
    /// exceeding its in-flight limit.
    ///
    /// The caller may supply its current report age, but a retry always uses at
    /// least the minimum age carried by the retry entry. This makes age
    /// monotonic across delivery attempts even if the caller accidentally
    /// reuses a stale age value.
    pub fn queue_ready_retries(
        &mut self,
        now_ms: u64,
        age_ms: u64,
        user_agent: &str,
    ) -> Vec<(FetchId, u32)> {
        let capacity = self.scheduler.limit().saturating_sub(self.scheduler.len());
        let ready = self.retries.drain_ready_up_to(now_ms, capacity);
        let mut queued = Vec::with_capacity(ready.len());

        for entry in ready {
            let retry_age_ms = age_ms.max(entry.minimum_age_ms);
            let id = self
                .scheduler
                .queue(entry.batch, retry_age_ms, user_agent)
                .expect("bounded Reporting API retry must fit scheduler capacity");
            self.attempts.insert(
                id,
                ReportingAttemptState {
                    attempt: entry.attempt,
                    age_ms: retry_age_ms,
                },
            );
            queued.push((id, entry.attempt));
        }
        queued
    }

    pub fn dispatch(&mut self, network: &dyn NetworkBackend) -> usize {
        self.scheduler.dispatch(network)
    }

    pub fn process_completions(
        &mut self,
        completions: Vec<FetchCompletion>,
        now_ms: u64,
    ) -> (Vec<ReportingRuntimeCompletion>, Vec<FetchCompletion>) {
        let (outcomes, unhandled) = self.scheduler.process_completions(completions);
        let mut completed = Vec::with_capacity(outcomes.len());

        for outcome in outcomes {
            let id = match &outcome {
                ReportingDeliveryOutcome::Delivered { id, .. }
                | ReportingDeliveryOutcome::Retryable { id, .. } => *id,
            };
            let state = self.attempts.remove(&id).unwrap_or(ReportingAttemptState {
                attempt: 1,
                age_ms: 0,
            });
            let retry = self.retries.schedule_outcome_with_age(
                outcome.clone(),
                state.attempt,
                now_ms,
                state.age_ms,
            );
            completed.push(ReportingRuntimeCompletion {
                attempt: state.attempt,
                outcome,
                retry,
            });
        }

        (completed, unhandled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrity_policy_reporting::{IntegrityViolationReport, IntegrityViolationReportBody};
    use crate::net::{ManualNetwork, Url};
    use crate::reporting_endpoints::ResolvedIntegrityViolationReport;
    use crate::reporting_scheduler::ReportingDeliveryFailure;

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
    fn failed_delivery_reenters_scheduler_after_backoff() {
        let network = ManualNetwork::new();
        network.respond_with("https://reports.test/collect", 503, "text/plain", Vec::new());
        network.set_auto_complete(true);

        let mut runtime = ReportingDeliveryRuntime::new(ReportingRetryPolicy::new(100, 1000, 3));
        runtime
            .queue_initial(batch("https://reports.test/collect"), 0, "ua")
            .unwrap();
        runtime.dispatch(&network);
        let (completed, unhandled) = runtime.process_completions(network.poll(), 1_000);
        assert!(unhandled.is_empty());
        assert_eq!(completed[0].attempt, 1);
        assert!(matches!(
            completed[0].outcome,
            ReportingDeliveryOutcome::Retryable {
                failure: ReportingDeliveryFailure::HttpStatus(503),
                ..
            }
        ));
        assert_eq!(runtime.retry_len(), 1);
        assert!(runtime.queue_ready_retries(1_099, 0, "ua").is_empty());

        let queued = runtime.queue_ready_retries(1_100, 0, "ua");
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].1, 2);
        assert_eq!(runtime.retry_len(), 0);
    }
}
