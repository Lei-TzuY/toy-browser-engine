use browser_engine::net::FetchResponse;
use browser_engine::{ReportingEndpoints, Url, REPORTING_ENDPOINTS_HEADER};

fn response_with_reporting_endpoints(value: &str) -> FetchResponse {
    let mut response = FetchResponse::synthetic(
        Url::parse("https://example.test/app/page.html").unwrap(),
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
fn structured_field_parameters_do_not_change_endpoint_resolution() {
    let response = response_with_reporting_endpoints(
        r#"primary="../reports";priority=high;persist=?1;sample=0.5, backup="https://reports.test/fallback";flag"#,
    );

    let endpoints = ReportingEndpoints::from_response(&response);
    assert_eq!(endpoints.len(), 2);
    assert_eq!(
        endpoints.get("primary").unwrap().to_string(),
        "https://example.test/reports"
    );
    assert_eq!(
        endpoints.get("backup").unwrap().to_string(),
        "https://reports.test/fallback"
    );
}

#[test]
fn quoted_parameter_values_can_contain_commas_and_semicolons() {
    let response = response_with_reporting_endpoints(
        r#"primary="/reports";note="edge,canary;west", backup="/backup""#,
    );

    let endpoints = ReportingEndpoints::from_response(&response);
    assert_eq!(endpoints.len(), 2);
    assert_eq!(
        endpoints.get("primary").unwrap().to_string(),
        "https://example.test/reports"
    );
}

#[test]
fn malformed_parameter_invalidates_the_structured_dictionary() {
    let response = response_with_reporting_endpoints(
        r#"primary="/reports";priority=@invalid, backup="/backup""#,
    );

    assert!(ReportingEndpoints::from_response(&response).is_empty());
}

#[test]
fn duplicate_parameter_key_invalidates_the_structured_dictionary() {
    let response = response_with_reporting_endpoints(
        r#"primary="/reports";flag;flag=?0, backup="/backup""#,
    );

    assert!(ReportingEndpoints::from_response(&response).is_empty());
}
