use browser_engine::{
    cross_origin_resource_policy_allows, parse_cross_origin_resource_policy,
    response_allows_cross_origin_resource, CorpOriginRelation, CrossOriginResourcePolicy,
};
use browser_engine::net::HeaderMap;

fn headers(values: &[&str]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for value in values {
        headers.append_raw("Cross-Origin-Resource-Policy", value);
    }
    headers
}

#[test]
fn public_parser_accepts_only_fetch_corp_tokens() {
    assert_eq!(
        parse_cross_origin_resource_policy(&headers(&["same-origin"])),
        Some(CrossOriginResourcePolicy::SameOrigin)
    );
    assert_eq!(
        parse_cross_origin_resource_policy(&headers(&["same-site"])),
        Some(CrossOriginResourcePolicy::SameSite)
    );
    assert_eq!(
        parse_cross_origin_resource_policy(&headers(&["cross-origin"])),
        Some(CrossOriginResourcePolicy::CrossOrigin)
    );
    assert_eq!(
        parse_cross_origin_resource_policy(&headers(&["Same-Origin"])),
        None
    );
}

#[test]
fn duplicate_or_combined_policy_fields_fail_open_under_unsafe_none() {
    let duplicate = headers(&["same-origin", "same-site"]);
    let combined = headers(&["same-origin, same-site"]);

    assert_eq!(parse_cross_origin_resource_policy(&duplicate), None);
    assert_eq!(parse_cross_origin_resource_policy(&combined), None);
    assert!(response_allows_cross_origin_resource(
        &duplicate,
        CorpOriginRelation::CrossSite,
        true,
        true
    ));
}

#[test]
fn same_origin_policy_requires_an_exact_origin_match() {
    let policy = Some(CrossOriginResourcePolicy::SameOrigin);
    assert!(cross_origin_resource_policy_allows(
        policy,
        CorpOriginRelation::SameOrigin,
        true,
        true
    ));
    assert!(!cross_origin_resource_policy_allows(
        policy,
        CorpOriginRelation::SameSite,
        true,
        true
    ));
    assert!(!cross_origin_resource_policy_allows(
        policy,
        CorpOriginRelation::CrossSite,
        true,
        true
    ));
}

#[test]
fn same_site_policy_blocks_cross_site_and_http_to_https_same_site() {
    let policy = Some(CrossOriginResourcePolicy::SameSite);

    assert!(cross_origin_resource_policy_allows(
        policy,
        CorpOriginRelation::SameSite,
        true,
        true
    ));
    assert!(cross_origin_resource_policy_allows(
        policy,
        CorpOriginRelation::SameSite,
        false,
        false
    ));
    assert!(!cross_origin_resource_policy_allows(
        policy,
        CorpOriginRelation::SameSite,
        false,
        true
    ));
    assert!(!cross_origin_resource_policy_allows(
        policy,
        CorpOriginRelation::CrossSite,
        true,
        true
    ));
}

#[test]
fn cross_origin_and_missing_policy_allow_ordinary_unsafe_none_fetches() {
    for policy in [None, Some(CrossOriginResourcePolicy::CrossOrigin)] {
        assert!(cross_origin_resource_policy_allows(
            policy,
            CorpOriginRelation::CrossSite,
            false,
            true
        ));
    }
}

#[test]
fn response_helper_combines_parsing_and_enforcement() {
    assert!(!response_allows_cross_origin_resource(
        &headers(&["same-origin"]),
        CorpOriginRelation::CrossSite,
        true,
        true
    ));
    assert!(response_allows_cross_origin_resource(
        &headers(&["cross-origin"]),
        CorpOriginRelation::CrossSite,
        true,
        true
    ));
}
