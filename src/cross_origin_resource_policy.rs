//! Cross-Origin-Resource-Policy (CORP) response enforcement for no-CORS
//! element subresource loads.
//!
//! CORP is a response-side isolation policy. A resource can opt into
//! `same-origin` or `same-site` embedding restrictions; `cross-origin` and an
//! absent/invalid policy preserve the normal permissive behavior. CORS-enabled
//! requests are governed by the CORS protocol instead, so callers should only
//! apply this check to the no-CORS element path.

use crate::net::{FetchError, FetchResponse, Origin, Url};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossOriginResourcePolicy {
    SameOrigin,
    SameSite,
    CrossOrigin,
}

/// Parse the CORP response header according to Fetch's case-sensitive tokens.
///
/// The Fetch grammar recognizes exactly one of the three lowercase keywords.
/// Invalid, empty, or combined values are ignored rather than guessed.
pub fn parse_cross_origin_resource_policy(
    response: &FetchResponse,
) -> Option<CrossOriginResourcePolicy> {
    match response.headers.get("cross-origin-resource-policy")?.as_str() {
        "same-origin" => Some(CrossOriginResourcePolicy::SameOrigin),
        "same-site" => Some(CrossOriginResourcePolicy::SameSite),
        "cross-origin" => Some(CrossOriginResourcePolicy::CrossOrigin),
        _ => None,
    }
}

/// Enforce CORP for one no-CORS subresource response.
///
/// `same_site` is supplied by the browser/session layer because this engine
/// intentionally keeps site computation (and future public-suffix-list work)
/// outside the network primitives. `same-origin` is computed exactly from URL
/// origins here. Missing/invalid policies and `cross-origin` allow the body.
pub fn validate_cross_origin_resource_policy(
    source: Option<&Url>,
    same_site: bool,
    response: &FetchResponse,
) -> Result<(), FetchError> {
    let Some(policy) = parse_cross_origin_resource_policy(response) else {
        return Ok(());
    };

    match policy {
        CrossOriginResourcePolicy::CrossOrigin => Ok(()),
        CrossOriginResourcePolicy::SameOrigin => {
            let Some(source) = source else {
                return Err(blocked("same-origin", response));
            };
            if Origin::of(source) == Origin::of(&response.url) {
                Ok(())
            } else {
                Err(blocked("same-origin", response))
            }
        }
        CrossOriginResourcePolicy::SameSite => {
            if source.is_some() && same_site {
                Ok(())
            } else {
                Err(blocked("same-site", response))
            }
        }
    }
}

fn blocked(policy: &str, response: &FetchResponse) -> FetchError {
    FetchError::Blocked(format!(
        "CORP: Cross-Origin-Resource-Policy {policy} blocked {}",
        response.url
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(input: &str) -> Url {
        Url::parse(input).unwrap()
    }

    fn response(url_value: &str, policy: Option<&str>) -> FetchResponse {
        let mut response = FetchResponse::synthetic(
            url(url_value),
            200,
            Some("application/octet-stream"),
            b"body".to_vec(),
        );
        if let Some(policy) = policy {
            response
                .headers
                .insert_raw("cross-origin-resource-policy", policy);
        }
        response
    }

    #[test]
    fn tokens_are_case_sensitive() {
        let upper = response("https://cdn.test/x", Some("Same-Origin"));
        assert_eq!(parse_cross_origin_resource_policy(&upper), None);
        let exact = response("https://cdn.test/x", Some("same-origin"));
        assert_eq!(
            parse_cross_origin_resource_policy(&exact),
            Some(CrossOriginResourcePolicy::SameOrigin)
        );
    }

    #[test]
    fn same_origin_allows_only_matching_origin() {
        let response = response("https://page.test:443/x", Some("same-origin"));
        assert!(validate_cross_origin_resource_policy(
            Some(&url("https://page.test/index.html")),
            true,
            &response,
        )
        .is_ok());
        assert!(validate_cross_origin_resource_policy(
            Some(&url("https://other.test/index.html")),
            false,
            &response,
        )
        .is_err());
    }

    #[test]
    fn same_site_uses_browser_classification() {
        let response = response("https://static.page.test/x", Some("same-site"));
        assert!(validate_cross_origin_resource_policy(
            Some(&url("https://page.test/index.html")),
            true,
            &response,
        )
        .is_ok());
        assert!(validate_cross_origin_resource_policy(
            Some(&url("https://page.test/index.html")),
            false,
            &response,
        )
        .is_err());
    }

    #[test]
    fn cross_origin_and_missing_policy_allow() {
        let source = url("https://page.test/index.html");
        assert!(validate_cross_origin_resource_policy(
            Some(&source),
            false,
            &response("https://cdn.test/x", Some("cross-origin")),
        )
        .is_ok());
        assert!(validate_cross_origin_resource_policy(
            Some(&source),
            false,
            &response("https://cdn.test/x", None),
        )
        .is_ok());
    }
}
