use browser_engine::net::FetchResponse;
use browser_engine::{ReportingEndpoints, Url, REPORTING_ENDPOINTS_HEADER};

fn response_with_reporting_endpoints(url: &str, value: &str) -> FetchResponse {
    let mut response = FetchResponse::synthetic(
        Url::parse(url).unwrap(),
        200,
        Some("text/html"),
        Vec::new(),
    );
    response
        .headers
        .insert_raw(REPORTING_ENDPOINTS_HEADER, value);
    response
}

#[test]
fn relative_reporting_endpoints_resolve_against_final_response_url() {
    let response = response_with_reporting_endpoints(
        "https://example.test/app/page.html?view=1#old",
        r#"root="/reports/integrity", sibling="../collector?kind=integrity#secret", protocol="//reports.test/report""#,
    );

    let endpoints = ReportingEndpoints::from_response(&response);
    assert_eq!(endpoints.len(), 3);
    assert_eq!(
        endpoints.get("root").unwrap().to_string(),
        "https://example.test/reports/integrity"
    );
    assert_eq!(
        endpoints.get("sibling").unwrap().to_string(),
        "https://example.test/collector?kind=integrity"
    );
    assert_eq!(
        endpoints.get("protocol").unwrap().to_string(),
        "https://reports.test/report"
    );
}

#[test]
fn untrustworthy_response_cannot_install_even_https_reporting_endpoint() {
    let response = response_with_reporting_endpoints(
        "http://example.test/page.html",
        r#"primary="https://reports.test/report""#,
    );

    assert!(ReportingEndpoints::from_response(&response).is_empty());
}

#[test]
fn loopback_http_response_and_endpoint_are_potentially_trustworthy() {
    let response = response_with_reporting_endpoints(
        "http://localhost:8000/app/page.html",
        r#"primary="../reports", remote="http://example.test/report""#,
    );

    let endpoints = ReportingEndpoints::from_response(&response);
    assert_eq!(endpoints.len(), 1);
    assert_eq!(
        endpoints.get("primary").unwrap().to_string(),
        "http://localhost:8000/reports"
    );
    assert!(endpoints.get("remote").is_none());
}

#[test]
fn malformed_absolute_member_does_not_poison_valid_relative_member() {
    let response = response_with_reporting_endpoints(
        "https://example.test/app/page.html",
        r#"broken="https://reports.test:bad/report", valid="./reports""#,
    );

    let endpoints = ReportingEndpoints::from_response(&response);
    assert!(endpoints.get("broken").is_none());
    assert_eq!(
        endpoints.get("valid").unwrap().to_string(),
        "https://example.test/app/reports"
    );
}

#[test]
fn header_only_parser_remains_absolute_only_without_a_response_base() {
    let response = response_with_reporting_endpoints(
        "https://example.test/app/page.html",
        r#"relative="/reports", absolute="https://reports.test/report""#,
    );

    let endpoints = ReportingEndpoints::from_headers(&response.headers);
    assert!(endpoints.get("relative").is_none());
    assert_eq!(
        endpoints.get("absolute").unwrap().to_string(),
        "https://reports.test/report"
    );
}
