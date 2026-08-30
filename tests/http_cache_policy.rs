use browser_engine::{cached_response_is_fresh, response_cache_policy};
use browser_engine::net::fetch::HeaderMap;

fn headers(value: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert_raw("cache-control", value);
    headers
}

#[test]
fn response_max_age_is_reusable_until_its_freshness_boundary() {
    let policy = response_cache_policy(&headers("public, max-age=120"));
    assert!(policy.storable);
    assert_eq!(policy.freshness_lifetime_secs, Some(120));
    assert!(cached_response_is_fresh(policy, 120));
    assert!(!cached_response_is_fresh(policy, 121));
}

#[test]
fn no_store_wins_over_other_permissive_directives() {
    let policy = response_cache_policy(&headers("public, max-age=3600, no-store"));
    assert!(!policy.storable);
    assert!(!cached_response_is_fresh(policy, 1));
}

#[test]
fn no_cache_can_store_but_cannot_reuse_without_validation() {
    let policy = response_cache_policy(&headers("private, max-age=3600, no-cache"));
    assert!(policy.storable);
    assert!(policy.requires_revalidation);
    assert!(!cached_response_is_fresh(policy, 0));
}

#[test]
fn conflicting_freshness_metadata_fails_safe() {
    let policy = response_cache_policy(&headers("max-age=60, max-age=300"));
    assert_eq!(policy.freshness_lifetime_secs, Some(0));
    assert!(policy.requires_revalidation);
}

#[test]
fn shared_cache_s_maxage_does_not_override_browser_private_max_age() {
    let policy = response_cache_policy(&headers("s-maxage=5, max-age=300"));
    assert_eq!(policy.freshness_lifetime_secs, Some(300));
    assert!(cached_response_is_fresh(policy, 200));
}

#[test]
fn directive_names_are_case_insensitive_and_quoted_delta_seconds_work() {
    let policy = response_cache_policy(&headers("PRIVATE, MAX-AGE=\"45\", MUST-REVALIDATE"));
    assert!(policy.storable);
    assert_eq!(policy.freshness_lifetime_secs, Some(45));
    assert!(policy.must_revalidate);
}
