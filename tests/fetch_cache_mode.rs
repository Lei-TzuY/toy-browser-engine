use browser_engine::script::host::RequestMode;
use browser_engine::{
    cache_mode_is_valid_for_request_mode, effective_cache_mode_for_headers, FetchCacheMode,
};

#[test]
fn web_idl_values_round_trip_and_are_case_sensitive() {
    for value in [
        "default",
        "no-store",
        "reload",
        "no-cache",
        "force-cache",
        "only-if-cached",
    ] {
        let parsed = FetchCacheMode::parse(value).expect("valid cache mode");
        assert_eq!(parsed.as_str(), value);
    }

    assert_eq!(FetchCacheMode::parse("DEFAULT"), None);
    assert_eq!(FetchCacheMode::parse(" only-if-cached "), None);
}

#[test]
fn only_if_cached_is_restricted_to_same_origin_mode() {
    assert!(cache_mode_is_valid_for_request_mode(
        FetchCacheMode::OnlyIfCached,
        RequestMode::SameOrigin,
    ));
    assert!(!cache_mode_is_valid_for_request_mode(
        FetchCacheMode::OnlyIfCached,
        RequestMode::Cors,
    ));
    assert!(!cache_mode_is_valid_for_request_mode(
        FetchCacheMode::OnlyIfCached,
        RequestMode::NoCors,
    ));
}

#[test]
fn conditional_request_headers_rewrite_only_default_mode() {
    assert_eq!(
        effective_cache_mode_for_headers(FetchCacheMode::Default, ["If-None-Match"]),
        FetchCacheMode::NoStore,
    );
    assert_eq!(
        effective_cache_mode_for_headers(FetchCacheMode::Default, ["IF-MODIFIED-SINCE"]),
        FetchCacheMode::NoStore,
    );
    assert_eq!(
        effective_cache_mode_for_headers(FetchCacheMode::ForceCache, ["If-Range"]),
        FetchCacheMode::ForceCache,
    );
    assert_eq!(
        effective_cache_mode_for_headers(FetchCacheMode::Default, ["Accept", "X-Test"]),
        FetchCacheMode::Default,
    );
}
