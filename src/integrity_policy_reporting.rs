//! Reporting payload construction for SRI `Integrity-Policy` violations.
//!
//! The SRI specification queues one `integrity-violation` report for every
//! endpoint named by the policy that actually violated. Enforced and
//! report-only policies are reported independently and differ through the
//! `reportOnly` bit in the report body.

use crate::integrity_policy::{
    IntegrityPolicy, IntegrityPolicyDecision, IntegrityPolicyDestination,
};
use crate::net::Url;

pub const INTEGRITY_VIOLATION_REPORT_TYPE: &str = "integrity-violation";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrityViolationReportBody {
    pub document_url: String,
    pub blocked_url: String,
    pub destination: String,
    pub report_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrityViolationReport {
    pub report_type: &'static str,
    pub endpoint: String,
    pub body: IntegrityViolationReportBody,
}

/// Build Reporting-API work items for one Integrity-Policy decision.
///
/// The caller can enqueue these records into a concrete Reporting API later.
/// Keeping queueing separate makes the SRI policy layer deterministic and
/// independently testable while preserving endpoint fan-out and report-only
/// semantics from the specification.
pub fn build_integrity_violation_reports(
    document_url: &Url,
    blocked_url: &Url,
    destination: IntegrityPolicyDestination,
    decision: IntegrityPolicyDecision,
    enforced: &IntegrityPolicy,
    report_only: &IntegrityPolicy,
) -> Vec<IntegrityViolationReport> {
    let document_url = strip_url_for_report(document_url);
    let blocked_url = strip_url_for_report(blocked_url);
    let destination = destination_token(destination).to_string();
    let mut reports = Vec::new();

    if decision.enforced_violation {
        reports.extend(enforced.endpoints.iter().cloned().map(|endpoint| {
            IntegrityViolationReport {
                report_type: INTEGRITY_VIOLATION_REPORT_TYPE,
                endpoint,
                body: IntegrityViolationReportBody {
                    document_url: document_url.clone(),
                    blocked_url: blocked_url.clone(),
                    destination: destination.clone(),
                    report_only: false,
                },
            }
        }));
    }

    if decision.report_only_violation {
        reports.extend(report_only.endpoints.iter().cloned().map(|endpoint| {
            IntegrityViolationReport {
                report_type: INTEGRITY_VIOLATION_REPORT_TYPE,
                endpoint,
                body: IntegrityViolationReportBody {
                    document_url: document_url.clone(),
                    blocked_url: blocked_url.clone(),
                    destination: destination.clone(),
                    report_only: true,
                },
            }
        }));
    }

    reports
}

fn destination_token(destination: IntegrityPolicyDestination) -> &'static str {
    match destination {
        IntegrityPolicyDestination::Script => "script",
        IntegrityPolicyDestination::Style => "style",
        IntegrityPolicyDestination::Other => "",
    }
}

fn strip_url_for_report(url: &Url) -> String {
    // Reporting payloads must not expose fragments. The engine's Url type does
    // not carry a separate report-URL abstraction yet, so keep the conservative
    // privacy boundary here and leave broader Reporting spec redaction for the
    // eventual shared Reporting API layer.
    let mut stripped = url.clone();
    stripped.set_fragment(None);
    stripped.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrity_policy::{evaluate_integrity_policy, IntegrityPolicyRequestMode};

    #[test]
    fn fans_out_enforced_and_report_only_endpoints_independently() {
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
        let reports = build_integrity_violation_reports(
            &Url::parse("https://example.test/page#secret").unwrap(),
            &Url::parse("https://cdn.test/app.js#fragment").unwrap(),
            IntegrityPolicyDestination::Script,
            decision,
            &enforced,
            &report_only,
        );

        assert_eq!(reports.len(), 3);
        assert_eq!(reports[0].endpoint, "primary");
        assert!(!reports[0].body.report_only);
        assert_eq!(reports[1].endpoint, "backup");
        assert!(!reports[1].body.report_only);
        assert_eq!(reports[2].endpoint, "observe");
        assert!(reports[2].body.report_only);
        assert_eq!(reports[0].report_type, "integrity-violation");
        assert_eq!(reports[0].body.destination, "script");
        assert_eq!(reports[0].body.document_url, "https://example.test/page");
        assert_eq!(reports[0].body.blocked_url, "https://cdn.test/app.js");
    }

    #[test]
    fn does_not_report_a_non_violation_or_policy_without_endpoints() {
        let empty = IntegrityPolicy::default();
        let reports = build_integrity_violation_reports(
            &Url::parse("https://example.test/").unwrap(),
            &Url::parse("https://example.test/app.js").unwrap(),
            IntegrityPolicyDestination::Script,
            IntegrityPolicyDecision::default(),
            &empty,
            &empty,
        );
        assert!(reports.is_empty());

        let enforced = IntegrityPolicy::parse("blocked-destinations=(script)");
        let decision = IntegrityPolicyDecision {
            blocked: true,
            enforced_violation: true,
            report_only_violation: false,
        };
        let reports = build_integrity_violation_reports(
            &Url::parse("https://example.test/").unwrap(),
            &Url::parse("https://cdn.test/app.js").unwrap(),
            IntegrityPolicyDestination::Script,
            decision,
            &enforced,
            &empty,
        );
        assert!(reports.is_empty());
    }
}
