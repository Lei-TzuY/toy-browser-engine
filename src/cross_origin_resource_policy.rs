//! Cross-Origin-Resource-Policy response-header parsing and enforcement.
//!
//! This module models Fetch's CORP internal check independently of a concrete
//! loader. The browser can use it before exposing an opaque/no-CORS response to
//! script or a subresource consumer.

use crate::net::HeaderMap;

/// Parsed `Cross-Origin-Resource-Policy` response policy.
///
/// Fetch defines these values case-sensitively. Invalid, combined, or duplicate
/// field values are treated as if no explicit CORP policy were supplied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossOriginResourcePolicy {
    SameOrigin,
    SameSite,
    CrossOrigin,
}

impl CrossOriginResourcePolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SameOrigin => "same-origin",
            Self::SameSite => "same-site",
            Self::CrossOrigin => "cross-origin",
        }
    }
}

/// Relationship between the request origin and the response URL origin.
///
/// `SameSite` here means Fetch's *schemelessly same site* relationship. Keeping
/// public-suffix/site computation outside this module lets the browser reuse
/// its existing site classifier while this policy primitive stays deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorpOriginRelation {
    SameOrigin,
    SameSite,
    CrossSite,
}

/// Parse one effective `Cross-Origin-Resource-Policy` response field.
///
/// Multiple field lines are deliberately invalid. Fetch obtains the header as
/// a single value; duplicate lines therefore combine into a value that matches
/// none of the three case-sensitive tokens.
pub fn parse_cross_origin_resource_policy(
    headers: &HeaderMap,
) -> Option<CrossOriginResourcePolicy> {
    let mut values = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("cross-origin-resource-policy"))
        .map(|(_, value)| trim_http_ows(value));

    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }

    match value {
        "same-origin" => Some(CrossOriginResourcePolicy::SameOrigin),
        "same-site" => Some(CrossOriginResourcePolicy::SameSite),
        "cross-origin" => Some(CrossOriginResourcePolicy::CrossOrigin),
        _ => None,
    }
}

/// Apply the explicit CORP policy for Fetch's ordinary `unsafe-none` embedder
/// policy case.
///
/// `initiator_is_https` and `response_is_https` encode Fetch's asymmetric
/// same-site downgrade guard: an HTTP initiator must not use `same-site` to read
/// a response delivered over HTTPS, even when the two origins are otherwise
/// schemelessly same-site.
pub fn cross_origin_resource_policy_allows(
    policy: Option<CrossOriginResourcePolicy>,
    relation: CorpOriginRelation,
    initiator_is_https: bool,
    response_is_https: bool,
) -> bool {
    match policy {
        None | Some(CrossOriginResourcePolicy::CrossOrigin) => true,
        Some(CrossOriginResourcePolicy::SameOrigin) => {
            relation == CorpOriginRelation::SameOrigin
        }
        Some(CrossOriginResourcePolicy::SameSite) => {
            relation != CorpOriginRelation::CrossSite
                && (initiator_is_https || !response_is_https)
        }
    }
}

/// Parse the response header and immediately apply the explicit CORP policy.
pub fn response_allows_cross_origin_resource(
    headers: &HeaderMap,
    relation: CorpOriginRelation,
    initiator_is_https: bool,
    response_is_https: bool,
) -> bool {
    cross_origin_resource_policy_allows(
        parse_cross_origin_resource_policy(headers),
        relation,
        initiator_is_https,
        response_is_https,
    )
}

fn trim_http_ows(value: &str) -> &str {
    value.trim_matches(|c| matches!(c, ' ' | '\t'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(values: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append_raw("Cross-Origin-Resource-Policy", value);
        }
        headers
    }

    #[test]
    fn parses_the_three_case_sensitive_tokens() {
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
    }

    #[test]
    fn invalid_case_combined_and_duplicate_values_are_not_policies() {
        assert_eq!(
            parse_cross_origin_resource_policy(&headers(&["Same-Origin"])),
            None
        );
        assert_eq!(
            parse_cross_origin_resource_policy(&headers(&["same-origin, same-site"])),
            None
        );
        assert_eq!(
            parse_cross_origin_resource_policy(&headers(&["same-origin", "same-origin"])),
            None
        );
    }

    #[test]
    fn http_optional_whitespace_around_a_single_value_is_ignored() {
        assert_eq!(
            parse_cross_origin_resource_policy(&headers(&[" \tsame-site\t "])),
            Some(CrossOriginResourcePolicy::SameSite)
        );
    }

    #[test]
    fn same_origin_policy_blocks_every_other_relation() {
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
    fn same_site_policy_honors_fetch_secure_transport_guard() {
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
    fn missing_invalid_and_cross_origin_policies_allow_under_unsafe_none() {
        assert!(cross_origin_resource_policy_allows(
            None,
            CorpOriginRelation::CrossSite,
            false,
            true
        ));
        assert!(cross_origin_resource_policy_allows(
            Some(CrossOriginResourcePolicy::CrossOrigin),
            CorpOriginRelation::CrossSite,
            false,
            true
        ));
    }
}
