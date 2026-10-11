use browser_engine::{
    batch_resolved_integrity_reports, build_reporting_delivery_requests,
    reporting_delivery_succeeded, IntegrityViolationReport, IntegrityViolationReportBody,
    ResolvedIntegrityViolationReport, Url, REPORTING_CONTENT_TYPE,
};

fn resolved(endpoint: &str, blocked: &str, report_only: bool) -> ResolvedIntegrityViolationReport {
    ResolvedIntegrityViolationReport {
        endpoint_name: "default".to_string(),
        endpoint_url: Url::parse(endpoint).unwrap(),
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
fn resolved_batches_become_post_requests_with_reporting_media_type() {
    let resolved = vec![
        resolved(
            "https://reports.test/collect",
            "https://cdn.test/a.js",
            false,
        ),
        resolved(
            "https://reports.test/collect",
            "https://cdn.test/b.js",
            true,
        ),
    ];
    let batches = batch_resolved_integrity_reports(&resolved);
    let requests = build_reporting_delivery_requests(&batches, 250, "toy-browser/test");

    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method.as_str(), "POST");
    assert_eq!(request.url.to_string(), "https://reports.test/collect");
    assert_eq!(
        request.headers.get("content-type").as_deref(),
        Some(REPORTING_CONTENT_TYPE)
    );

    // Reporting transport is browser-generated. It must not accidentally inherit
    // ambient document request headers at this construction boundary.
    assert!(!request.headers.has("cookie"));
    assert!(!request.headers.has("authorization"));
    assert!(!request.headers.has("origin"));
    assert!(!request.headers.has("referer"));

    let body = String::from_utf8(request.body.clone().unwrap()).unwrap();
    assert!(body.starts_with("["));
    assert!(body.ends_with("]"));
    assert_eq!(body.matches("\"type\":\"integrity-violation\"").count(), 2);
    assert!(body.contains("\"age\":250"));
    assert!(body.contains("\"user_agent\":\"toy-browser/test\""));
    assert!(body.contains("\"reportOnly\":false"));
    assert!(body.contains("\"reportOnly\":true"));
}

#[test]
fn concrete_endpoint_order_is_preserved_into_transport_requests() {
    let resolved = vec![
        resolved("https://reports.test/a", "https://cdn.test/1.js", false),
        resolved("https://reports.test/b", "https://cdn.test/2.js", false),
        resolved("https://reports.test/a", "https://cdn.test/3.js", false),
    ];
    let batches = batch_resolved_integrity_reports(&resolved);
    let requests = build_reporting_delivery_requests(&batches, 0, "ua");

    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].url.to_string(), "https://reports.test/a");
    assert_eq!(requests[1].url.to_string(), "https://reports.test/b");

    let first_body = String::from_utf8(requests[0].body.clone().unwrap()).unwrap();
    let first = first_body.find("https://cdn.test/1.js").unwrap();
    let third = first_body.find("https://cdn.test/3.js").unwrap();
    assert!(first < third);
}

#[test]
fn delivery_status_classification_accepts_only_2xx() {
    for status in [200, 201, 204, 299] {
        assert!(reporting_delivery_succeeded(status), "status {status}");
    }
    for status in [0, 199, 300, 404, 500] {
        assert!(!reporting_delivery_succeeded(status), "status {status}");
    }
}
