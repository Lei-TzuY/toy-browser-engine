//! Network request construction for Reporting API delivery batches.
//!
//! `reporting_delivery` owns grouping and JSON serialization. This module turns
//! one serialized batch into the concrete HTTP request that can be handed to a
//! transport without going through script Fetch request construction.

use crate::net::{FetchRequest, HeaderMap, Method};
use crate::reporting_delivery::{ReportingDeliveryBatch, REPORTING_CONTENT_TYPE};

/// Build one Reporting API delivery request.
///
/// The request is intentionally constructed from a fresh header list. Reporting
/// delivery is browser-generated traffic, so authored page headers, cookies,
/// Origin, and Referer state are not inherited here. A higher-level scheduler
/// may add browser-owned transport policy, but must not merge script headers
/// into this request.
pub fn build_reporting_delivery_request(
    batch: &ReportingDeliveryBatch,
    age_ms: u64,
    user_agent: &str,
) -> FetchRequest {
    let mut headers = HeaderMap::new();
    headers.insert_raw("content-type", REPORTING_CONTENT_TYPE);

    FetchRequest::new(
        batch.endpoint_url.clone(),
        Method::Post,
        headers,
        Some(batch.to_json(age_ms, user_agent).into_bytes()),
    )
}

/// Build requests for all delivery batches while preserving batch order.
pub fn build_reporting_delivery_requests(
    batches: &[ReportingDeliveryBatch],
    age_ms: u64,
    user_agent: &str,
) -> Vec<FetchRequest> {
    batches
        .iter()
        .map(|batch| build_reporting_delivery_request(batch, age_ms, user_agent))
        .collect()
}

/// Reporting delivery treats any 2xx response as successful submission.
pub fn reporting_delivery_succeeded(status: u16) -> bool {
    (200..300).contains(&status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrity_policy_reporting::{IntegrityViolationReport, IntegrityViolationReportBody};
    use crate::net::Url;
    use crate::reporting_endpoints::ResolvedIntegrityViolationReport;

    fn batch(endpoint: &str) -> ReportingDeliveryBatch {
        ReportingDeliveryBatch {
            endpoint_url: Url::parse(endpoint).unwrap(),
            reports: vec![ResolvedIntegrityViolationReport {
                endpoint_name: "default".to_string(),
                endpoint_url: Url::parse(endpoint).unwrap(),
                report: IntegrityViolationReport {
                    report_type: "integrity-violation",
                    endpoint: "default".to_string(),
                    body: IntegrityViolationReportBody {
                        document_url: "https://example.test/page".to_string(),
                        blocked_url: "https://cdn.test/app.js".to_string(),
                        destination: "script".to_string(),
                        report_only: false,
                    },
                },
            }],
        }
    }

    #[test]
    fn constructs_browser_owned_post_request() {
        let request = build_reporting_delivery_request(
            &batch("https://reports.test/collect"),
            42,
            "toy-browser/1.0",
        );

        assert_eq!(request.url.to_string(), "https://reports.test/collect");
        assert_eq!(request.method, Method::Post);
        assert_eq!(
            request.headers.get("content-type").as_deref(),
            Some(REPORTING_CONTENT_TYPE)
        );
        assert_eq!(request.headers.len(), 1);
        assert!(!request.headers.has("cookie"));
        assert!(!request.headers.has("origin"));
        assert!(!request.headers.has("referer"));

        let body = String::from_utf8(request.body.unwrap()).unwrap();
        assert!(body.contains("\"age\":42"));
        assert!(body.contains("\"user_agent\":\"toy-browser/1.0\""));
        assert!(body.contains("\"type\":\"integrity-violation\""));
    }

    #[test]
    fn preserves_batch_order_when_building_requests() {
        let batches = vec![
            batch("https://reports.test/first"),
            batch("https://reports.test/second"),
        ];
        let requests = build_reporting_delivery_requests(&batches, 0, "ua");
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].url.to_string(), "https://reports.test/first");
        assert_eq!(requests[1].url.to_string(), "https://reports.test/second");
    }

    #[test]
    fn recognizes_only_success_status_class() {
        assert!(reporting_delivery_succeeded(200));
        assert!(reporting_delivery_succeeded(204));
        assert!(reporting_delivery_succeeded(299));
        assert!(!reporting_delivery_succeeded(199));
        assert!(!reporting_delivery_succeeded(300));
        assert!(!reporting_delivery_succeeded(500));
    }
}
