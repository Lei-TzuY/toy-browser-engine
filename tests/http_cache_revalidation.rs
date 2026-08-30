use browser_engine::{
    cache_revalidation_headers, response_cache_validators, response_confirms_not_modified,
    HttpCacheValidators,
};
use browser_engine::net::fetch::HeaderMap;

#[test]
fn cached_etag_and_last_modified_become_conditional_request_fields() {
    let mut response_headers = HeaderMap::new();
    response_headers.insert_raw("etag", "W/\"asset-42\"");
    response_headers.insert_raw("last-modified", "Sun, 30 Aug 2026 10:00:00 GMT");

    let validators = response_cache_validators(&response_headers);
    let request_headers = cache_revalidation_headers(&validators);

    assert_eq!(
        request_headers.get("if-none-match").as_deref(),
        Some("W/\"asset-42\"")
    );
    assert_eq!(
        request_headers.get("if-modified-since").as_deref(),
        Some("Sun, 30 Aug 2026 10:00:00 GMT")
    );
}

#[test]
fn etag_only_revalidation_does_not_invent_a_date_condition() {
    let headers = cache_revalidation_headers(&HttpCacheValidators {
        etag: Some("\"build-9\"".into()),
        last_modified: None,
    });
    assert_eq!(headers.get("if-none-match").as_deref(), Some("\"build-9\""));
    assert!(headers.get("if-modified-since").is_none());
}

#[test]
fn last_modified_only_revalidation_does_not_invent_an_etag() {
    let headers = cache_revalidation_headers(&HttpCacheValidators {
        etag: None,
        last_modified: Some("Sun, 30 Aug 2026 10:00:00 GMT".into()),
    });
    assert!(headers.get("if-none-match").is_none());
    assert_eq!(
        headers.get("if-modified-since").as_deref(),
        Some("Sun, 30 Aug 2026 10:00:00 GMT")
    );
}

#[test]
fn validator_extraction_is_case_insensitive_through_header_map() {
    let mut headers = HeaderMap::new();
    headers.insert_raw("ETag", "\"CaseFolded\"");
    headers.insert_raw("Last-Modified", "Sun, 30 Aug 2026 11:00:00 GMT");

    let validators = response_cache_validators(&headers);
    assert_eq!(validators.etag.as_deref(), Some("\"CaseFolded\""));
    assert_eq!(
        validators.last_modified.as_deref(),
        Some("Sun, 30 Aug 2026 11:00:00 GMT")
    );
}

#[test]
fn only_status_304_marks_cached_representation_not_modified() {
    assert!(response_confirms_not_modified(304));
    assert!(!response_confirms_not_modified(200));
    assert!(!response_confirms_not_modified(412));
}
