//! Stateful Reporting API coordination across endpoint resolution and delivery.
//!
//! `ReportingEndpointState` owns the response-committed endpoint mapping and the
//! `410 Gone` removals learned from prior deliveries. `ReportingDeliveryRuntime`
//! owns transport scheduling and retry timing. This layer deliberately composes
//! those responsibilities so callers cannot accidentally observe a
//! `RemoveEndpoint` disposition and then forget to apply it before resolving the
//! next report.

use crate::integrity_policy_reporting::IntegrityViolationReport;
use crate::net::{FetchCompletion, FetchError, FetchId, NetworkBackend};
use crate::reporting_delivery::{
    batch_resolved_integrity_reports, ReportingDeliveryBatch,
};
use crate::reporting_endpoint_state::ReportingEndpointState;
use crate::reporting_endpoints::ReportingEndpoints;
use crate::reporting_retry::ReportingRetryPolicy;
use crate::reporting_runtime::{ReportingDeliveryRuntime, ReportingRuntimeCompletion};

/// End-to-end Reporting API state for one committed endpoint mapping.
///
/// The coordinator keeps endpoint-removal state synchronized with terminal
/// delivery outcomes while leaving the browser event loop in charge of when
/// network completions are polled and when ready retries are dispatched.
#[derive(Debug)]
pub struct ReportingCoordinator {
    endpoints: ReportingEndpointState,
    runtime: ReportingDeliveryRuntime,
}

impl ReportingCoordinator {
    pub fn new(endpoints: ReportingEndpoints, retry_policy: ReportingRetryPolicy) -> Self {
        Self {
            endpoints: ReportingEndpointState::new(endpoints),
            runtime: ReportingDeliveryRuntime::new(retry_policy),
        }
    }

    pub fn with_in_flight_limit(mut self, limit: usize) -> Self {
        self.runtime = self.runtime.with_in_flight_limit(limit);
        self
    }

    /// The live response-committed endpoint state, including URLs removed by a
    /// prior `410 Gone` delivery.
    pub fn endpoint_state(&self) -> &ReportingEndpointState {
        &self.endpoints
    }

    /// Replace the mapping after a new response commits Reporting-Endpoints.
    ///
    /// Removal state belongs to the mapping that learned it, so committing a
    /// fresh mapping intentionally clears prior removals. Already in-flight
    /// deliveries remain owned by the transport runtime and are not cancelled.
    pub fn replace_endpoints(&mut self, endpoints: ReportingEndpoints) {
        self.endpoints = ReportingEndpointState::new(endpoints);
    }

    /// Resolve reports through the current endpoint state and group them into
    /// concrete delivery batches. Removed endpoints are excluded before any
    /// network work is created.
    pub fn resolve_and_batch(
        &self,
        reports: &[IntegrityViolationReport],
    ) -> Vec<ReportingDeliveryBatch> {
        let resolved = self.endpoints.resolve(reports);
        batch_resolved_integrity_reports(&resolved)
    }

    /// Queue one already-resolved delivery batch as attempt 1.
    ///
    /// A stale caller cannot re-queue a concrete endpoint after this
    /// coordinator has applied a `410 Gone` removal for it.
    pub fn queue_initial_batch(
        &mut self,
        batch: ReportingDeliveryBatch,
        age_ms: u64,
        user_agent: &str,
    ) -> Result<FetchId, FetchError> {
        if self.endpoints.is_removed(&batch.endpoint_url) {
            return Err(FetchError::BadRequest(format!(
                "Reporting endpoint {} was removed by a prior 410 response",
                batch.endpoint_url
            )));
        }
        self.runtime.queue_initial(batch, age_ms, user_agent)
    }

    pub fn queue_ready_retries(
        &mut self,
        now_ms: u64,
        age_ms: u64,
        user_agent: &str,
    ) -> Vec<(FetchId, u32)> {
        self.runtime
            .queue_ready_retries(now_ms, age_ms, user_agent)
    }

    pub fn dispatch(&mut self, network: &dyn NetworkBackend) -> usize {
        self.runtime.dispatch(network)
    }

    pub fn in_flight_len(&self) -> usize {
        self.runtime.in_flight_len()
    }

    pub fn retry_len(&self) -> usize {
        self.runtime.retry_len()
    }

    pub fn is_idle(&self) -> bool {
        self.runtime.is_idle()
    }

    /// Route completions through the delivery runtime and immediately apply any
    /// terminal endpoint-removal dispositions before returning to the caller.
    ///
    /// The third tuple element is the number of concrete endpoint URLs newly
    /// removed by this completion batch. Non-reporting completions are returned
    /// untouched in the second tuple element.
    pub fn process_completions(
        &mut self,
        completions: Vec<FetchCompletion>,
        now_ms: u64,
    ) -> (Vec<ReportingRuntimeCompletion>, Vec<FetchCompletion>, usize) {
        let (completed, unhandled) = self.runtime.process_completions(completions, now_ms);
        let removed = self.apply_endpoint_outcomes(&completed);
        (completed, unhandled, removed)
    }

    /// Deterministic wall-clock variant of [`Self::process_completions`].
    pub fn process_completions_at(
        &mut self,
        completions: Vec<FetchCompletion>,
        now_ms: u64,
        now_unix_ms: u64,
    ) -> (Vec<ReportingRuntimeCompletion>, Vec<FetchCompletion>, usize) {
        let (completed, unhandled) =
            self.runtime
                .process_completions_at(completions, now_ms, now_unix_ms);
        let removed = self.apply_endpoint_outcomes(&completed);
        (completed, unhandled, removed)
    }

    fn apply_endpoint_outcomes(&mut self, completed: &[ReportingRuntimeCompletion]) -> usize {
        let outcomes = completed
            .iter()
            .map(|completion| completion.outcome.clone())
            .collect::<Vec<_>>();
        self.endpoints.apply_delivery_outcomes(&outcomes)
    }
}
