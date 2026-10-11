use browser_engine::{ReportingEndpoints, Url};

#[test]
fn non_ascii_endpoint_string_invalidates_structured_dictionary() {
    let endpoints = ReportingEndpoints::parse(
        "default=\"https://reports.test/café\", backup=\"https://reports.test/backup\"",
    );
    assert!(endpoints.is_empty());
}

#[test]
fn non_ascii_parameter_string_invalidates_structured_dictionary() {
    let endpoints = ReportingEndpoints::parse(
        "default=\"https://reports.test/collect\";label=\"café\"",
    );
    assert!(endpoints.is_empty());
}

#[test]
fn percent_encoded_utf8_remains_valid_ascii_structured_string() {
    let endpoints = ReportingEndpoints::parse(
        "default=\"https://reports.test/caf%C3%A9\";label=\"edge\"",
    );
    assert_eq!(
        endpoints.get("default").unwrap().to_string(),
        "https://reports.test/caf%C3%A9"
    );
}

#[test]
fn ascii_relative_reference_still_resolves_against_response_url() {
    let base = Url::parse("https://example.test/app/page.html").unwrap();
    let endpoints = ReportingEndpoints::parse_with_base(
        "default=\"../reports?kind=csp\";label=\"edge\"",
        &base,
    );
    assert_eq!(
        endpoints.get("default").unwrap().to_string(),
        "https://example.test/reports?kind=csp"
    );
}
