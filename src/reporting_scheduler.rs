//! Browser-owned Reporting API delivery scheduling.
//!
//! Reporting delivery must not share script Fetch bookkeeping: report requests
//! are browser-generated work and may outlive the JavaScript operation that
//! caused them. This module gives reporting traffic its own in-flight id space,
//! dispatch queue, and completion classification while reusing the common
//! `NetworkBackend` transport boundary.

use crate::net::{FetchCompletion, FetchError, FetchId, FetchRequest, NetworkBackend};
use crate::reporting_delivery::ReportingDeliveryBatch;
use crate::reporting_request::{build_reporting_delivery_request, reporting_delivery_succeeded};

/// Keep browser-owned report ids out of the ordinary page Fetch id range.
const REPORTING_FETCH_ID_BASE: FetchId = 1 << 63;

/// Default maximum number of concurrently pending Reporting API deliveries.
pub const MAX_IN_FLIGHT_REPORTING_DELIVERIES: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportingDeliveryFailure {
    HttpStatus(u16),
    Network(FetchError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportingDeliveryOutcome {
    Delivered {
        id: FetchId,
        batch: ReportingDeliveryBatch,
    },
    Retryable {
        id: FetchId,
        batch: ReportingDeliveryBatch,
        failure: ReportingDeliveryFailure,
    },
}

/// Small transport scheduler dedicated to Reporting API traffic.
///
/// Queuing is separate from dispatch so callers can enqueue reports while the
/// browser is processing a document task and hand them to the network only at
/// the normal network phase of the event loop.
#[derive(Debug)]
pub struct ReportingDeliveryScheduler {
    next_id: FetchId,
    pending: Vec<(FetchId, ReportingDeliveryBatch)>,
    outbox: Vec<(FetchId, FetchRequest)>,
    limit: usize,
}

impl Default for ReportingDeliveryScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl ReportingDeliveryScheduler {
    pub fn new() -> Self {
        Self {
            next_id: REPORTING_FETCH_ID_BASE,
            pending: Vec::new(),
            outbox: Vec::new(),
            limit: MAX_IN_FLIGHT_REPORTING_DELIVERIES,
        }
    }

    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit.max(1);
        self
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Queue one already-resolved delivery batch.
    ///
    /// The body is serialized at queue time so the caller-provided age and user
    /// agent describe this concrete delivery attempt. A future retry scheduler
    /// can deliberately requeue the returned batch with a newer age.
    pub fn queue(
        &mut self,
        batch: ReportingDeliveryBatch,
        age_ms: u64,
        user_agent: &str,
    ) -> Result<FetchId, FetchError> {
        if self.pending.len() >= self.limit {
            return Err(FetchError::TooManyRequests);
        }

        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        if self.next_id < REPORTING_FETCH_ID_BASE {
            self.next_id = REPORTING_FETCH_ID_BASE;
        }

        let request = build_reporting_delivery_request(&batch, age_ms, user_agent);
        self.pending.push((id, batch));
        self.outbox.push((id, request));
        Ok(id)
    }

    /// Dispatch all queued requests through the browser's network backend.
    pub fn dispatch(&mut self, network: &dyn NetworkBackend) -> usize {
        let outbox = std::mem::take(&mut self.outbox);
        let count = outbox.len();
        for (id, request) in outbox {
            network.start(id, request);
        }
        count
    }

    /// Route completions that the browser event loop has already polled.
    ///
    /// This intentionally does **not** call `NetworkBackend::poll()` itself.
    /// Reporting and script Fetch may share a backend, and a reporting helper
    /// must never steal a page completion merely by polling first. Completions
    /// whose ids do not belong to this scheduler are returned untouched so the
    /// event loop can route them to the page Fetch registry.
    ///
    /// Successful 2xx responses consume the batch. Network failures and
    /// non-2xx responses return the original batch so a higher-level policy can
    /// retry it without reconstructing report contents.
    pub fn process_completions(
        &mut self,
        completions: Vec<FetchCompletion>,
    ) -> (Vec<ReportingDeliveryOutcome>, Vec<FetchCompletion>) {
        let mut outcomes = Vec::new();
        let mut unhandled = Vec::new();

        for completion in completions {
            let Some(index) = self
                .pending
                .iter()
                .position(|(id, _)| *id == completion.id)
            else {
                unhandled.push(completion);
                continue;
            };
            let (_, batch) = self.pending.remove(index);
            let outcome = match completion.result {
                Ok(response) if reporting_delivery_succeeded(response.status) => {
                    ReportingDeliveryOutcome::Delivered {
                        id: completion.id,
                        batch,
                    }
                }
                Ok(response) => ReportingDeliveryOutcome::Retryable {
                    id: completion.id,
                    batch,
                    failure: ReportingDeliveryFailure::HttpStatus(response.status),
                },
                Err(error) => ReportingDeliveryOutcome::Retryable {
                    id: completion.id,
                    batch,
                    failure: ReportingDeliveryFailure::Network(error),
                },
            };
            outcomes.push(outcome);
        }
        (outcomes, unhandled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrity_policy_reporting::{IntegrityViolationReport, IntegrityViolationReportBody};
    use crate::net::{HeaderMap, ManualNetwork, Method, Url};
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
    fn uses_browser_owned_id_space_and_dispatches_post() {
        let network = ManualNetwork::new();
        let mut scheduler = ReportingDeliveryScheduler::new();
        let id = scheduler
            .queue(batch("https://reports.test/collect"), 12, "toy/1")
            .unwrap();
        assert!(id >= REPORTING_FETCH_ID_BASE);
        assert_eq!(scheduler.dispatch(&network), 1);
        let requests = network.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method.as_str(), "POST");
        assert_eq!(requests[0].url.to_string(), "https://reports.test/collect");
    }

    #[test]
    fn success_consumes_pending_batch() {
        let network = ManualNetwork::new();
        network.respond_with("https://reports.test/collect", 204, "text/plain", Vec::new());
        network.set_auto_complete(true);
        let mut scheduler = ReportingDeliveryScheduler::new();
        scheduler
            .queue(batch("https://reports.test/collect"), 0, "ua")
            .unwrap();
        scheduler.dispatch(&network);
        let (outcomes, unhandled) = scheduler.process_completions(network.poll());
        assert!(matches!(outcomes.as_slice(), [ReportingDeliveryOutcome::Delivered { .. }]));
        assert!(unhandled.is_empty());
        assert!(scheduler.is_empty());
    }

    #[test]
    fn non_success_returns_batch_for_retry() {
        let network = ManualNetwork::new();
        network.respond_with("https://reports.test/collect", 503, "text/plain", Vec::new());
        network.set_auto_complete(true);
        let mut scheduler = ReportingDeliveryScheduler::new();
        scheduler
            .queue(batch("https://reports.test/collect"), 0, "ua")
            .unwrap();
        scheduler.dispatch(&network);
        let (outcomes, unhandled) = scheduler.process_completions(network.poll());
        assert!(unhandled.is_empty());
        assert!(matches!(
            outcomes.as_slice(),
            [ReportingDeliveryOutcome::Retryable {
                failure: ReportingDeliveryFailure::HttpStatus(503),
                ..
            }]
        ));
        assert!(scheduler.is_empty());
    }

    #[test]
    fn network_failure_returns_batch_for_retry() {
        let network = ManualNetwork::new();
        network.fail("https://reports.test/collect", FetchError::Timeout("endpoint".into()));
        network.set_auto_complete(true);
        let mut scheduler = ReportingDeliveryScheduler::new();
        scheduler
            .queue(batch("https://reports.test/collect"), 0, "ua")
            .unwrap();
        scheduler.dispatch(&network);
        let (outcomes, unhandled) = scheduler.process_completions(network.poll());
        assert!(unhandled.is_empty());
        assert!(matches!(
            outcomes.as_slice(),
            [ReportingDeliveryOutcome::Retryable {
                failure: ReportingDeliveryFailure::Network(FetchError::Timeout(_)),
                ..
            }]
        ));
    }

    #[test]
    fn enforces_separate_reporting_in_flight_limit() {
        let mut scheduler = ReportingDeliveryScheduler::new().with_limit(1);
        scheduler
            .queue(batch("https://reports.test/one"), 0, "ua")
            .unwrap();
        assert_eq!(
            scheduler.queue(batch("https://reports.test/two"), 0, "ua"),
            Err(FetchError::TooManyRequests)
        );
    }

    #[test]
    fn returns_unrelated_page_fetch_completions_untouched() {
        let network = ManualNetwork::new();
        network.respond_with("https://example.test/data", 200, "text/plain", b"ok".to_vec());
        network.set_auto_complete(true);
        network.start(
            1,
            FetchRequest::new(
                Url::parse("https://example.test/data").unwrap(),
                Method::Get,
                HeaderMap::new(),
                None,
            ),
        );
        let mut scheduler = ReportingDeliveryScheduler::new();
        let completions = network.poll();
        let (outcomes, unhandled) = scheduler.process_completions(completions);
        assert!(outcomes.is_empty());
        assert_eq!(unhandled.len(), 1);
        assert_eq!(unhandled[0].id, 1);
    }
}
