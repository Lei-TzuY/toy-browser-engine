//! Mutable Reporting API endpoint state.
//!
//! `ReportingEndpoints` represents the mapping committed by a response. Delivery
//! can later invalidate a concrete endpoint when that endpoint returns
//! `410 Gone`. This layer owns that lifecycle without coupling parsing to the
//! transport scheduler.

use crate::integrity_policy_reporting::IntegrityViolationReport;
use crate::net::Url;
use crate::reporting_endpoints::{
    resolve_integrity_violation_reports, ReportingEndpoints, ResolvedIntegrityViolationReport,
};
use crate::reporting_scheduler::{ReportingDeliveryDisposition, ReportingDeliveryOutcome};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReportingEndpointState {
    endpoints: ReportingEndpoints,
    removed_urls: Vec<Url>,
}

impl ReportingEndpointState {
    pub fn new(endpoints: ReportingEndpoints) -> Self {
        Self {
            endpoints,
            removed_urls: Vec::new(),
        }
    }

    pub fn endpoints(&self) -> &ReportingEndpoints {
        &self.endpoints
    }

    pub fn removed_len(&self) -> usize {
        self.removed_urls.len()
    }

    pub fn is_removed(&self, url: &Url) -> bool {
        self.removed_urls.iter().any(|removed| removed == url)
    }

    /// Apply terminal delivery outcomes to endpoint state.
    ///
    /// A normal 2xx delivery leaves endpoint state unchanged. `410 Gone` is
    /// represented by `RemoveEndpoint`; every alias in the committed mapping
    /// that resolves to the same concrete URL is then suppressed from future
    /// report resolution. Re-applying the same removal is idempotent.
    pub fn apply_delivery_outcomes(&mut self, outcomes: &[ReportingDeliveryOutcome]) -> usize {
        let mut removed = 0;
        for outcome in outcomes {
            let ReportingDeliveryOutcome::Delivered {
                batch,
                disposition: ReportingDeliveryDisposition::RemoveEndpoint,
                ..
            } = outcome
            else {
                continue;
            };

            if !self.is_removed(&batch.endpoint_url) {
                self.removed_urls.push(batch.endpoint_url.clone());
                removed += 1;
            }
        }
        removed
    }

    /// Resolve reports against the committed mapping after applying any
    /// endpoint-removal state accumulated from prior deliveries.
    pub fn resolve(
        &self,
        reports: &[IntegrityViolationReport],
    ) -> Vec<ResolvedIntegrityViolationReport> {
        resolve_integrity_violation_reports(reports, &self.endpoints)
            .into_iter()
            .filter(|resolved| !self.is_removed(&resolved.endpoint_url))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrity_policy_reporting::{
        IntegrityViolationReport, IntegrityViolationReportBody,
    };
    use crate::reporting_delivery::ReportingDeliveryBatch;
    use crate::reporting_scheduler::ReportingDeliveryOutcome;

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

    #[test]
    fn remove_endpoint_suppresses_all_aliases_for_same_url() {
        let endpoints = ReportingEndpoints::parse(
            r#"primary="https://reports.test/collect", backup="https://reports.test/collect", other="https://reports.test/other""#,
        );
        let mut state = ReportingEndpointState::new(endpoints);
        let reports = vec![
            report("primary", "https://cdn.test/a.js"),
            report("backup", "https://cdn.test/b.js"),
            report("other", "https://cdn.test/c.js"),
        ];
        let resolved = state.resolve(&reports);
        assert_eq!(resolved.len(), 3);

        let removed_url = Url::parse("https://reports.test/collect").unwrap();
        let outcome = ReportingDeliveryOutcome::Delivered {
            id: 1,
            batch: ReportingDeliveryBatch {
                endpoint_url: removed_url.clone(),
                reports: vec![resolved[0].clone()],
            },
            disposition: ReportingDeliveryDisposition::RemoveEndpoint,
        };

        assert_eq!(state.apply_delivery_outcomes(&[outcome.clone()]), 1);
        assert_eq!(state.apply_delivery_outcomes(&[outcome]), 0);
        assert!(state.is_removed(&removed_url));

        let remaining = state.resolve(&reports);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].endpoint_name, "other");
    }

    #[test]
    fn ordinary_delivery_does_not_remove_endpoint() {
        let endpoints = ReportingEndpoints::parse(
            r#"default="https://reports.test/collect""#,
        );
        let mut state = ReportingEndpointState::new(endpoints);
        let reports = vec![report("default", "https://cdn.test/a.js")];
        let resolved = state.resolve(&reports);
        let outcome = ReportingDeliveryOutcome::Delivered {
            id: 2,
            batch: ReportingDeliveryBatch {
                endpoint_url: Url::parse("https://reports.test/collect").unwrap(),
                reports: resolved.clone(),
            },
            disposition: ReportingDeliveryDisposition::Delivered,
        };

        assert_eq!(state.apply_delivery_outcomes(&[outcome]), 0);
        assert_eq!(state.resolve(&reports).len(), 1);
    }
}
