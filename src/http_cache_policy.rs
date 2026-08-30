//! HTTP response cache-policy primitives for the browser's private cache.
//!
//! This module models the RFC 9111 decisions that can be made from a response's
//! `Cache-Control` field without requiring a concrete storage backend. It is
//! intentionally private-cache focused: `private` is therefore cacheable, while
//! `s-maxage` is ignored because it targets shared caches.

use crate::net::fetch::HeaderMap;

/// Cache policy derived from one HTTP response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpResponseCachePolicy {
    /// Whether a private user-agent cache may store the response.
    pub storable: bool,
    /// Whether a stored response must be validated before reuse.
    pub requires_revalidation: bool,
    /// Explicit freshness lifetime from `max-age`, if valid and unambiguous.
    pub freshness_lifetime_secs: Option<u64>,
    /// Whether stale reuse is forbidden without successful validation.
    pub must_revalidate: bool,
}

impl Default for HttpResponseCachePolicy {
    fn default() -> Self {
        Self {
            storable: true,
            requires_revalidation: false,
            freshness_lifetime_secs: None,
            must_revalidate: false,
        }
    }
}

/// Derive private-cache policy from response headers.
///
/// RFC 9111 treats `no-store` as a storage prohibition and `no-cache` as a
/// requirement to validate before reuse. `max-age` supplies explicit freshness.
/// Invalid or conflicting `max-age` values are treated conservatively as stale,
/// which means the representation may be stored but must be revalidated.
pub fn response_cache_policy(headers: &HeaderMap) -> HttpResponseCachePolicy {
    let Some(value) = headers.get("cache-control") else {
        return HttpResponseCachePolicy::default();
    };

    let mut policy = HttpResponseCachePolicy::default();
    let mut max_age: Option<u64> = None;
    let mut max_age_invalid_or_conflicting = false;

    for directive in split_cache_control_directives(&value) {
        let (name, argument) = split_directive(directive);
        match name.as_str() {
            "no-store" => policy.storable = false,
            "no-cache" => policy.requires_revalidation = true,
            "must-revalidate" => policy.must_revalidate = true,
            "max-age" => match argument.and_then(parse_delta_seconds) {
                Some(seconds) => {
                    if let Some(existing) = max_age {
                        if existing != seconds {
                            max_age_invalid_or_conflicting = true;
                        }
                    } else {
                        max_age = Some(seconds);
                    }
                }
                None => max_age_invalid_or_conflicting = true,
            },
            // `private` is intentionally allowed in a browser/private cache.
            // `s-maxage` and unknown extensions do not affect this policy.
            _ => {}
        }
    }

    if max_age_invalid_or_conflicting {
        policy.freshness_lifetime_secs = Some(0);
        policy.requires_revalidation = true;
    } else {
        policy.freshness_lifetime_secs = max_age;
    }

    policy
}

/// Return whether a stored response is still fresh at `current_age_secs`.
///
/// A response carrying `no-cache` is never reusable without validation even if
/// its explicit freshness lifetime has not elapsed.
pub fn cached_response_is_fresh(
    policy: HttpResponseCachePolicy,
    current_age_secs: u64,
) -> bool {
    if !policy.storable || policy.requires_revalidation {
        return false;
    }
    policy
        .freshness_lifetime_secs
        .is_some_and(|lifetime| current_age_secs <= lifetime)
}

fn split_cache_control_directives(value: &str) -> Vec<&str> {
    // Cache-Control arguments relevant here are either tokens or quoted scalar
    // values. A tiny quote-aware splitter is enough to avoid treating commas in
    // extension quoted strings as directive separators.
    let mut directives = Vec::new();
    let mut start = 0;
    let mut in_quotes = false;
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => in_quotes = !in_quotes,
            b',' if !in_quotes => {
                directives.push(value[start..i].trim());
                start = i + 1;
            }
            b'\\' if in_quotes && i + 1 < bytes.len() => i += 1,
            _ => {}
        }
        i += 1;
    }
    directives.push(value[start..].trim());
    directives.into_iter().filter(|d| !d.is_empty()).collect()
}

fn split_directive(directive: &str) -> (String, Option<&str>) {
    match directive.split_once('=') {
        Some((name, value)) => (name.trim().to_ascii_lowercase(), Some(value.trim())),
        None => (directive.trim().to_ascii_lowercase(), None),
    }
}

fn parse_delta_seconds(value: &str) -> Option<u64> {
    let value = value.trim();
    let unquoted = if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
        &value[1..value.len() - 1]
    } else {
        value
    };
    if unquoted.is_empty() || !unquoted.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    unquoted.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(cache_control: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert_raw("cache-control", cache_control);
        headers
    }

    #[test]
    fn no_store_forbids_storage() {
        let policy = response_cache_policy(&headers("public, max-age=600, no-store"));
        assert!(!policy.storable);
        assert_eq!(policy.freshness_lifetime_secs, Some(600));
        assert!(!cached_response_is_fresh(policy, 1));
    }

    #[test]
    fn no_cache_requires_validation_even_while_fresh() {
        let policy = response_cache_policy(&headers("max-age=600, no-cache"));
        assert!(policy.storable);
        assert!(policy.requires_revalidation);
        assert_eq!(policy.freshness_lifetime_secs, Some(600));
        assert!(!cached_response_is_fresh(policy, 10));
    }

    #[test]
    fn max_age_drives_freshness_boundary() {
        let policy = response_cache_policy(&headers("max-age=60"));
        assert!(cached_response_is_fresh(policy, 59));
        assert!(cached_response_is_fresh(policy, 60));
        assert!(!cached_response_is_fresh(policy, 61));
    }

    #[test]
    fn quoted_max_age_is_accepted() {
        let policy = response_cache_policy(&headers("max-age=\"120\""));
        assert_eq!(policy.freshness_lifetime_secs, Some(120));
    }

    #[test]
    fn invalid_max_age_is_conservatively_stale() {
        for value in ["max-age=", "max-age=abc", "max-age=-1"] {
            let policy = response_cache_policy(&headers(value));
            assert_eq!(policy.freshness_lifetime_secs, Some(0), "{value}");
            assert!(policy.requires_revalidation, "{value}");
        }
    }

    #[test]
    fn conflicting_duplicate_max_age_is_stale() {
        let policy = response_cache_policy(&headers("max-age=60, max-age=120"));
        assert_eq!(policy.freshness_lifetime_secs, Some(0));
        assert!(policy.requires_revalidation);
    }

    #[test]
    fn identical_duplicate_max_age_remains_usable() {
        let policy = response_cache_policy(&headers("max-age=60, MAX-AGE=60"));
        assert_eq!(policy.freshness_lifetime_secs, Some(60));
        assert!(cached_response_is_fresh(policy, 30));
    }

    #[test]
    fn private_is_cacheable_for_a_user_agent_cache() {
        let policy = response_cache_policy(&headers("private, max-age=30"));
        assert!(policy.storable);
        assert!(cached_response_is_fresh(policy, 30));
    }

    #[test]
    fn s_maxage_does_not_override_private_cache_max_age() {
        let policy = response_cache_policy(&headers("s-maxage=1, max-age=90"));
        assert_eq!(policy.freshness_lifetime_secs, Some(90));
    }

    #[test]
    fn must_revalidate_is_preserved_for_stale_handling() {
        let policy = response_cache_policy(&headers("max-age=10, must-revalidate"));
        assert!(policy.must_revalidate);
        assert!(cached_response_is_fresh(policy, 5));
        assert!(!cached_response_is_fresh(policy, 11));
    }

    #[test]
    fn quoted_extension_commas_do_not_split_directives() {
        let policy = response_cache_policy(&headers(
            "example=\"one,two\", max-age=45, private",
        ));
        assert_eq!(policy.freshness_lifetime_secs, Some(45));
    }

    #[test]
    fn missing_cache_control_has_no_explicit_freshness() {
        let policy = response_cache_policy(&HeaderMap::new());
        assert!(policy.storable);
        assert_eq!(policy.freshness_lifetime_secs, None);
        assert!(!cached_response_is_fresh(policy, 0));
    }
}
