use browser_engine::cross_origin_resource_policy::{
    response_allows_cross_origin_resource_with_embedder_policy, CorpOriginRelation,
    CrossOriginEmbedderPolicy,
};
use browser_engine::net::fetch::HeaderMap;

fn headers(corp: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(value) = corp {
        headers.append_raw("Cross-Origin-Resource-Policy", value);
    }
    headers
}

#[test]
fn require_corp_blocks_cross_origin_response_without_opt_in() {
    assert!(!response_allows_cross_origin_resource_with_embedder_policy(
        &headers(None),
        CrossOriginEmbedderPolicy::RequireCorp,
        CorpOriginRelation::CrossSite,
        true,
        true,
        false,
        false,
    ));

    assert!(response_allows_cross_origin_resource_with_embedder_policy(
        &headers(Some("cross-origin")),
        CrossOriginEmbedderPolicy::RequireCorp,
        CorpOriginRelation::CrossSite,
        true,
        true,
        false,
        false,
    ));
}

#[test]
fn credentialless_allows_uncredentialed_cross_origin_response_without_header() {
    assert!(response_allows_cross_origin_resource_with_embedder_policy(
        &headers(None),
        CrossOriginEmbedderPolicy::Credentialless,
        CorpOriginRelation::CrossSite,
        true,
        true,
        false,
        false,
    ));

    assert!(!response_allows_cross_origin_resource_with_embedder_policy(
        &headers(None),
        CrossOriginEmbedderPolicy::Credentialless,
        CorpOriginRelation::CrossSite,
        true,
        true,
        true,
        false,
    ));
}

#[test]
fn explicit_same_site_still_applies_secure_transport_guard() {
    assert!(!response_allows_cross_origin_resource_with_embedder_policy(
        &headers(Some("same-site")),
        CrossOriginEmbedderPolicy::Credentialless,
        CorpOriginRelation::SameSite,
        false,
        true,
        false,
        false,
    ));
}

#[test]
fn invalid_header_falls_back_to_embedder_policy_default() {
    assert!(!response_allows_cross_origin_resource_with_embedder_policy(
        &headers(Some("Same-Origin")),
        CrossOriginEmbedderPolicy::RequireCorp,
        CorpOriginRelation::CrossSite,
        true,
        true,
        false,
        false,
    ));
}

#[test]
fn unsafe_none_navigation_bypasses_corp_internal_check() {
    assert!(response_allows_cross_origin_resource_with_embedder_policy(
        &headers(Some("same-origin")),
        CrossOriginEmbedderPolicy::UnsafeNone,
        CorpOriginRelation::CrossSite,
        true,
        true,
        true,
        true,
    ));
}
