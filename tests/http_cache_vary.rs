use browser_engine::{parse_vary, vary_matches, HttpVary};

fn headers(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

#[test]
fn vary_without_fields_allows_reuse() {
    assert!(vary_matches(&[], &headers(&[("Accept", "text/html")]), &[]));
}

#[test]
fn vary_field_partitions_cached_responses() {
    let response = headers(&[("Vary", "Accept-Encoding")]);
    let stored = headers(&[("Accept-Encoding", "gzip")]);

    assert!(vary_matches(
        &response,
        &stored,
        &headers(&[("accept-encoding", "gzip")])
    ));
    assert!(!vary_matches(
        &response,
        &stored,
        &headers(&[("Accept-Encoding", "br")])
    ));
}

#[test]
fn multiple_vary_fields_must_all_match() {
    let response = headers(&[("Vary", "Accept-Encoding, Accept-Language")]);
    let stored = headers(&[("Accept-Encoding", "gzip"), ("Accept-Language", "en")]);

    assert!(vary_matches(
        &response,
        &stored,
        &headers(&[("Accept-Encoding", "gzip"), ("Accept-Language", "en")])
    ));
    assert!(!vary_matches(
        &response,
        &stored,
        &headers(&[("Accept-Encoding", "gzip"), ("Accept-Language", "fr")])
    ));
}

#[test]
fn wildcard_vary_is_never_reusable() {
    let response = headers(&[("Vary", "Accept-Encoding, *")]);
    assert_eq!(parse_vary(&response), HttpVary::Any);
    assert!(!vary_matches(&response, &[], &[]));
}

#[test]
fn malformed_field_name_fails_closed() {
    let response = headers(&[("Vary", "Accept Encoding")]);
    assert_eq!(parse_vary(&response), HttpVary::Any);
    assert!(!vary_matches(&response, &[], &[]));
}
