//! Session-local queue for pending SRI `Integrity-Policy` reports.
//!
//! `integrity_policy_reporting` constructs deterministic report records. This
//! module gives the browser a concrete ownership boundary for those records so
//! policy code can enqueue violations without coupling itself to transport or a
//! future Reporting API delivery backend.

use std::collections::VecDeque;

use crate::integrity_policy::{
    IntegrityPolicy, IntegrityPolicyDecision, IntegrityPolicyDestination,
};
use crate::integrity_policy_reporting::{
    build_integrity_violation_reports, IntegrityViolationReport,
};
use crate::net::Url;

/// Pending Integrity-Policy reports owned by one document/session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IntegrityReportQueue {
    pending: VecDeque<IntegrityViolationReport>,
}

impl IntegrityReportQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Queue every report implied by one Integrity-Policy decision.
    ///
    /// Returns the number of records added. Enforced and report-only endpoint
    /// fan-out is delegated to the reporting primitive from the previous stack
    /// layer, keeping ordering stable: enforced endpoints first, then
    /// report-only endpoints.
    pub fn enqueue_decision(
        &mut self,
        document_url: &Url,
        blocked_url: &Url,
        destination: IntegrityPolicyDestination,
        decision: IntegrityPolicyDecision,
        enforced: &IntegrityPolicy,
        report_only: &IntegrityPolicy,
    ) -> usize {
        let reports = build_integrity_violation_reports(
            document_url,
            blocked_url,
            destination,
            decision,
            enforced,
            report_only,
        );
        let added = reports.len();
        self.pending.extend(reports);
        added
    }

    /// Pop the oldest pending report.
    pub fn pop_front(&mut self) -> Option<IntegrityViolationReport> {
        self.pending.pop_front()
    }

    /// Drain all currently pending reports in enqueue order.
    pub fn drain(&mut self) -> Vec<IntegrityViolationReport> {
        self.pending.drain(..).collect()
    }

    /// Drain reports for one Reporting API endpoint while preserving the
    /// relative order of all unmatched records.
    ///
    /// This is useful for an eventual endpoint delivery scheduler: failure or
    /// backoff for one endpoint does not require disturbing another endpoint's
    /// pending work.
    pub fn drain_endpoint(&mut self, endpoint: &str) -> Vec<IntegrityViolationReport> {
        let mut matched = Vec::new();
        let mut retained = VecDeque::with_capacity(self.pending.len());
        while let Some(report) = self.pending.pop_front() {
            if report.endpoint == endpoint {
                matched.push(report);
            } else {
                retained.push_back(report);
            }
        }
        self.pending = retained;
        matched
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrity_policy::{evaluate_integrity_policy, IntegrityPolicyRequestMode};

    fn policies() -> (IntegrityPolicy, IntegrityPolicy, IntegrityPolicyDecision) {
        let enforced = IntegrityPolicy::parse(
            "blocked-destinations=(script), endpoints=(primary backup)",
        );
        let report_only = IntegrityPolicy::parse(
            "blocked-destinations=(script), endpoints=(observe)",
        );
        let decision = evaluate_integrity_policy(
            &enforced,
            &report_only,
            IntegrityPolicyDestination::Script,
            false,
            IntegrityPolicyRequestMode::NoCors,
            false,
        );
        (enforced, report_only, decision)
    }

    #[test]
    fn queues_fanout_in_stable_order() {
        let (enforced, report_only, decision) = policies();
        let mut queue = IntegrityReportQueue::new();
        let added = queue.enqueue_decision(
            &Url::parse("https://site.test/page#fragment").unwrap(),
            &Url::parse("https://cdn.test/app.js#hash").unwrap(),
            IntegrityPolicyDestination::Script,
            decision,
            &enforced,
            &report_only,
        );

        assert_eq!(added, 3);
        assert_eq!(queue.len(), 3);
        let reports = queue.drain();
        assert_eq!(reports.iter().map(|r| r.endpoint.as_str()).collect::<Vec<_>>(), vec!["primary", "backup", "observe"]);
        assert!(!reports[0].body.report_only);
        assert!(reports[2].body.report_only);
        assert_eq!(reports[0].body.document_url, "https://site.test/page");
    }

    #[test]
    fn endpoint_drain_preserves_other_report_order() {
        let (enforced, report_only, decision) = policies();
        let mut queue = IntegrityReportQueue::new();
        for resource in ["a.js", "b.js"] {
            queue.enqueue_decision(
                &Url::parse("https://site.test/").unwrap(),
                &Url::parse(&format!("https://cdn.test/{resource}")).unwrap(),
                IntegrityPolicyDestination::Script,
                decision,
                &enforced,
                &report_only,
            );
        }

        let primary = queue.drain_endpoint("primary");
        assert_eq!(primary.len(), 2);
        assert!(primary[0].body.blocked_url.ends_with("/a.js"));
        assert!(primary[1].body.blocked_url.ends_with("/b.js"));

        let remaining = queue.drain();
        assert_eq!(remaining.iter().map(|r| r.endpoint.as_str()).collect::<Vec<_>>(), vec!["backup", "observe", "backup", "observe"]);
    }

    #[test]
    fn non_violation_enqueues_nothing() {
        let mut queue = IntegrityReportQueue::new();
        let empty = IntegrityPolicy::default();
        assert_eq!(
            queue.enqueue_decision(
                &Url::parse("https://site.test/").unwrap(),
                &Url::parse("https://site.test/app.js").unwrap(),
                IntegrityPolicyDestination::Script,
                IntegrityPolicyDecision::default(),
                &empty,
                &empty,
            ),
            0
        );
        assert!(queue.is_empty());
        assert!(queue.pop_front().is_none());
    }
}
