//! Reporting API delivery batching for resolved Integrity-Policy violations.
//!
//! The policy/reporting layers intentionally stop short of doing network I/O.
//! This module provides the next deterministic boundary: reports that have
//! already been resolved through `Reporting-Endpoints` are grouped by concrete
//! endpoint and serialized into Reporting API JSON arrays suitable for an HTTP
//! POST body.

use crate::net::Url;
use crate::reporting_endpoints::ResolvedIntegrityViolationReport;

pub const REPORTING_CONTENT_TYPE: &str = "application/reports+json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportingDeliveryBatch {
    pub endpoint_url: Url,
    pub reports: Vec<ResolvedIntegrityViolationReport>,
}

impl ReportingDeliveryBatch {
    pub fn len(&self) -> usize {
        self.reports.len()
    }

    pub fn is_empty(&self) -> bool {
        self.reports.is_empty()
    }

    /// Serialize this batch as an `application/reports+json` array.
    ///
    /// `age_ms` is supplied by the eventual queue scheduler, which owns enqueue
    /// timestamps. `user_agent` is likewise supplied by the browser/session
    /// boundary instead of being guessed inside the policy layer.
    pub fn to_json(&self, age_ms: u64, user_agent: &str) -> String {
        let mut out = String::from("[");
        for (index, resolved) in self.reports.iter().enumerate() {
            if index != 0 {
                out.push(',');
            }
            let report = &resolved.report;
            out.push_str("{\"age\":");
            out.push_str(&age_ms.to_string());
            out.push_str(",\"type\":");
            push_json_string(&mut out, report.report_type);
            out.push_str(",\"url\":");
            push_json_string(&mut out, &report.body.document_url);
            out.push_str(",\"user_agent\":");
            push_json_string(&mut out, user_agent);
            out.push_str(",\"body\":{");
            out.push_str("\"documentURL\":");
            push_json_string(&mut out, &report.body.document_url);
            out.push_str(",\"blockedURL\":");
            push_json_string(&mut out, &report.body.blocked_url);
            out.push_str(",\"destination\":");
            push_json_string(&mut out, &report.body.destination);
            out.push_str(",\"reportOnly\":");
            out.push_str(if report.body.report_only { "true" } else { "false" });
            out.push_str("}}");
        }
        out.push(']');
        out
    }
}

/// Group resolved reports by concrete endpoint URL while preserving first-seen
/// endpoint order and FIFO order within each endpoint.
pub fn batch_resolved_integrity_reports(
    reports: &[ResolvedIntegrityViolationReport],
) -> Vec<ReportingDeliveryBatch> {
    let mut batches: Vec<ReportingDeliveryBatch> = Vec::new();

    for report in reports {
        if let Some(batch) = batches
            .iter_mut()
            .find(|batch| batch.endpoint_url == report.endpoint_url)
        {
            batch.reports.push(report.clone());
        } else {
            batches.push(ReportingDeliveryBatch {
                endpoint_url: report.endpoint_url.clone(),
                reports: vec![report.clone()],
            });
        }
    }

    batches
}

fn push_json_string(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch <= '\u{1F}' => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{:04x}", ch as u32);
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrity_policy_reporting::{IntegrityViolationReport, IntegrityViolationReportBody};

    fn resolved(endpoint: &str, blocked: &str, report_only: bool) -> ResolvedIntegrityViolationReport {
        let endpoint_url = Url::parse(endpoint).unwrap();
        ResolvedIntegrityViolationReport {
            endpoint_name: "default".to_string(),
            endpoint_url,
            report: IntegrityViolationReport {
                report_type: "integrity-violation",
                endpoint: "default".to_string(),
                body: IntegrityViolationReportBody {
                    document_url: "https://example.test/page".to_string(),
                    blocked_url: blocked.to_string(),
                    destination: "script".to_string(),
                    report_only,
                },
            },
        }
    }

    #[test]
    fn batches_by_concrete_endpoint_without_reordering_fifo() {
        let input = vec![
            resolved("https://reports.test/a", "https://cdn.test/1.js", false),
            resolved("https://reports.test/b", "https://cdn.test/2.js", false),
            resolved("https://reports.test/a", "https://cdn.test/3.js", true),
        ];
        let batches = batch_resolved_integrity_reports(&input);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].endpoint_url.to_string(), "https://reports.test/a");
        assert_eq!(batches[0].reports.len(), 2);
        assert_eq!(batches[0].reports[0].report.body.blocked_url, "https://cdn.test/1.js");
        assert_eq!(batches[0].reports[1].report.body.blocked_url, "https://cdn.test/3.js");
        assert_eq!(batches[1].endpoint_url.to_string(), "https://reports.test/b");
    }

    #[test]
    fn serializes_reporting_api_shape_and_escapes_strings() {
        let input = vec![resolved(
            "https://reports.test/a",
            "https://cdn.test/app.js?x=\"quoted\"&line=one\ntwo",
            true,
        )];
        let batch = batch_resolved_integrity_reports(&input).remove(0);
        let json = batch.to_json(125, "toy-browser/1.0 \"test\"");

        assert!(json.starts_with("[{\"age\":125,\"type\":\"integrity-violation\""));
        assert!(json.contains("\"url\":\"https://example.test/page\""));
        assert!(json.contains("\"user_agent\":\"toy-browser/1.0 \\\"test\\\"\""));
        assert!(json.contains("\"blockedURL\":\"https://cdn.test/app.js?x=\\\"quoted\\\"&line=one\\ntwo\""));
        assert!(json.contains("\"reportOnly\":true"));
        assert!(json.ends_with("]"));
    }

    #[test]
    fn empty_input_produces_no_delivery_batches() {
        assert!(batch_resolved_integrity_reports(&[]).is_empty());
    }
}
