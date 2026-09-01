use std::cell::RefCell;
use std::rc::{Rc, Weak};

use crate::cookie::CookieJar;
use crate::cookie_network::{CookieCredentials, CookieJarRef, CookieRequestPolicy};
use crate::cookie_same_site::SameSiteRequestContext;
use crate::fetch_cors::{is_cors_safelisted_method, validate_cors_response_origin};
use crate::net::{FetchError, FetchRequest, FetchResponse, HeaderMap, Method, Url};

const DEFAULT_CORS_PREFLIGHT_MAX_AGE_SECS: u64 = 5;
const MAX_CORS_PREFLIGHT_MAX_AGE_SECS: u64 = 7_200;
const MAX_CORS_PREFLIGHT_CACHE_ENTRIES_PER_SESSION: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
enum CorsPreflightPermission {
    Method(String),
    HeaderName(String),
}

#[derive(Debug, Clone)]
struct CorsPreflightCacheEntry {
    source_origin: String,
    target_url: String,
    credentialed: bool,
    permission: CorsPreflightPermission,
    expires_at_ms: u64,
}

#[derive(Debug)]
struct CorsPreflightSessionCache {
    jar: Weak<RefCell<CookieJar>>,
    entries: Vec<CorsPreflightCacheEntry>,
}

thread_local! {
    static CORS_PREFLIGHT_CACHES: RefCell<Vec<CorsPreflightSessionCache>> = RefCell::new(Vec::new());
}

fn with_session_cache<R>(
    jar: &CookieJarRef,
    operation: impl FnOnce(&mut Vec<CorsPreflightCacheEntry>) -> R,
) -> R {
    CORS_PREFLIGHT_CACHES.with(|sessions| {
        let mut sessions = sessions.borrow_mut();
        sessions.retain(|session| session.jar.upgrade().is_some());
        let index = sessions
            .iter()
            .position(|session| {
                session
                    .jar
                    .upgrade()
                    .is_some_and(|live| Rc::ptr_eq(&live, jar))
            })
            .unwrap_or_else(|| {
                sessions.push(CorsPreflightSessionCache {
                    jar: Rc::downgrade(jar),
                    entries: Vec::new(),
                });
                sessions.len() - 1
            });
        operation(&mut sessions[index].entries)
    })
}

fn effective_now_ms(jar: &CookieJarRef, now_ms: u64) -> u64 {
    jar.borrow().effective_now_ms(now_ms)
}

fn target_cache_key(target: &Url) -> String {
    target.without_fragment().to_string()
}

fn is_cors_non_wildcard_request_header_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("authorization")
}

fn comma_tokens(value: Option<String>) -> Vec<String> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_max_age_seconds(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }

    let mut parsed = 0_u64;
    for byte in value.bytes() {
        parsed = parsed
            .saturating_mul(10)
            .saturating_add(u64::from(byte - b'0'));
        if parsed >= MAX_CORS_PREFLIGHT_MAX_AGE_SECS {
            return Some(MAX_CORS_PREFLIGHT_MAX_AGE_SECS);
        }
    }
    Some(parsed)
}

fn max_age_seconds(response: &FetchResponse) -> u64 {
    response
        .headers
        .get("access-control-max-age")
        .and_then(|value| parse_max_age_seconds(&value))
        .unwrap_or(DEFAULT_CORS_PREFLIGHT_MAX_AGE_SECS)
}

fn cache_entry_matches_request(
    entry: &CorsPreflightCacheEntry,
    source_origin: &str,
    target_url: &str,
    credentialed: bool,
) -> bool {
    entry.source_origin == source_origin
        && entry.target_url == target_url
        && (entry.credentialed || !credentialed)
}

fn method_permission_matches(
    permission: &CorsPreflightPermission,
    requested_method: Method,
    credentialed: bool,
) -> bool {
    matches!(
        permission,
        CorsPreflightPermission::Method(method)
            if method == requested_method.as_str() || (method == "*" && !credentialed)
    )
}

fn header_permission_matches(
    permission: &CorsPreflightPermission,
    requested_header: &str,
    credentialed: bool,
) -> bool {
    matches!(
        permission,
        CorsPreflightPermission::HeaderName(header)
            if header.eq_ignore_ascii_case(requested_header)
                || (header == "*"
                    && !credentialed
                    && !is_cors_non_wildcard_request_header_name(requested_header))
    )
}

pub(crate) fn cache_allows(
    jar: &CookieJarRef,
    now_ms: u64,
    source_origin: &str,
    target: &Url,
    credentialed: bool,
    requested_method: Method,
    requested_headers: &[String],
) -> bool {
    let now_ms = effective_now_ms(jar, now_ms);
    let target_url = target_cache_key(target);

    with_session_cache(jar, |entries| {
        entries.retain(|entry| entry.expires_at_ms > now_ms);

        let method_allowed = is_cors_safelisted_method(requested_method)
            || entries.iter().any(|entry| {
                cache_entry_matches_request(entry, source_origin, &target_url, credentialed)
                    && method_permission_matches(&entry.permission, requested_method, credentialed)
            });
        if !method_allowed {
            return false;
        }

        requested_headers.iter().all(|requested| {
            entries.iter().any(|entry| {
                cache_entry_matches_request(entry, source_origin, &target_url, credentialed)
                    && header_permission_matches(&entry.permission, requested, credentialed)
            })
        })
    })
}

fn permission_matches_grant(
    entry: &CorsPreflightCacheEntry,
    permission: &CorsPreflightPermission,
    credentialed: bool,
) -> bool {
    match permission {
        CorsPreflightPermission::Method(method) => Method::parse(method).is_some_and(|parsed| {
            method_permission_matches(&entry.permission, parsed, credentialed)
        }) || matches!(
            &entry.permission,
            CorsPreflightPermission::Method(existing) if existing == method
        ),
        CorsPreflightPermission::HeaderName(header) => {
            header_permission_matches(&entry.permission, header, credentialed)
        }
    }
}

fn store_permission(
    entries: &mut Vec<CorsPreflightCacheEntry>,
    source_origin: &str,
    target_url: &str,
    credentialed: bool,
    permission: CorsPreflightPermission,
    expires_at_ms: u64,
) {
    if let Some(entry) = entries.iter_mut().find(|entry| {
        cache_entry_matches_request(entry, source_origin, target_url, credentialed)
            && permission_matches_grant(entry, &permission, credentialed)
    }) {
        entry.expires_at_ms = expires_at_ms;
        return;
    }

    // A zero max-age refreshes matching entries to immediate expiry, but there
    // is no value in allocating a brand-new entry that is already expired.
    if expires_at_ms == 0 {
        return;
    }

    if entries.len() >= MAX_CORS_PREFLIGHT_CACHE_ENTRIES_PER_SESSION {
        entries.remove(0);
    }
    entries.push(CorsPreflightCacheEntry {
        source_origin: source_origin.to_string(),
        target_url: target_url.to_string(),
        credentialed,
        permission,
        expires_at_ms,
    });
}

pub(crate) fn store_permissions(
    jar: &CookieJarRef,
    now_ms: u64,
    source_origin: &str,
    target: &Url,
    credentialed: bool,
    response: &FetchResponse,
) {
    let max_age = max_age_seconds(response);
    let now_ms = effective_now_ms(jar, now_ms);
    let target_url = target_cache_key(target);
    let expires_at_ms = now_ms.saturating_add(max_age.saturating_mul(1_000));
    let allowed_methods = comma_tokens(response.headers.get("access-control-allow-methods"));
    let allowed_headers = comma_tokens(response.headers.get("access-control-allow-headers"));

    with_session_cache(jar, |entries| {
        entries.retain(|entry| entry.expires_at_ms > now_ms);

        for method in allowed_methods {
            store_permission(
                entries,
                source_origin,
                &target_url,
                credentialed,
                CorsPreflightPermission::Method(method),
                expires_at_ms,
            );
        }
        for header in allowed_headers {
            store_permission(
                entries,
                source_origin,
                &target_url,
                credentialed,
                CorsPreflightPermission::HeaderName(header.to_ascii_lowercase()),
                expires_at_ms,
            );
        }
    });
}

pub(crate) fn build_preflight_request(
    target: Url,
    source_origin: &str,
    requested_method: Method,
    requested_headers: &[String],
) -> FetchRequest {
    let mut headers = HeaderMap::new();
    headers.insert_raw("origin", source_origin);
    headers.insert_raw("access-control-request-method", requested_method.as_str());
    if !requested_headers.is_empty() {
        headers.insert_raw(
            "access-control-request-headers",
            &requested_headers.join(","),
        );
    }
    FetchRequest::new(target, Method::Options, headers, None)
}

pub(crate) fn preflight_cookie_policy() -> CookieRequestPolicy {
    CookieRequestPolicy::new(
        CookieCredentials::Omit,
        SameSiteRequestContext::cross_site_subresource(Method::Options),
    )
}

pub(crate) fn validate_preflight_response(
    source_origin: &str,
    credentialed: bool,
    requested_method: Method,
    requested_headers: &[String],
    response: &FetchResponse,
) -> Result<(), FetchError> {
    if response.redirected {
        return Err(FetchError::Blocked(
            "CORS: redirected preflight responses are not supported".into(),
        ));
    }
    if !(200..300).contains(&response.status) {
        return Err(FetchError::Blocked(format!(
            "CORS: preflight response status {} is not successful",
            response.status
        )));
    }
    validate_cors_response_origin(source_origin, credentialed, response)?;

    if !is_cors_safelisted_method(requested_method) {
        let methods = comma_tokens(response.headers.get("access-control-allow-methods"));
        let wildcard = !credentialed && methods.iter().any(|method| method == "*");
        let exact = methods
            .iter()
            .any(|method| method == requested_method.as_str());
        if !wildcard && !exact {
            return Err(FetchError::Blocked(format!(
                "CORS: preflight did not allow method {}",
                requested_method
            )));
        }
    }

    if !requested_headers.is_empty() {
        let allowed = comma_tokens(response.headers.get("access-control-allow-headers"));
        let wildcard = !credentialed && allowed.iter().any(|header| header == "*");
        for requested in requested_headers {
            let wildcard_allows = wildcard && !is_cors_non_wildcard_request_header_name(requested);
            if !wildcard_allows
                && !allowed
                    .iter()
                    .any(|header| header.eq_ignore_ascii_case(requested))
            {
                return Err(FetchError::Blocked(format!(
                    "CORS: preflight did not allow request header {requested}"
                )));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jar() -> CookieJarRef {
        Rc::new(RefCell::new(CookieJar::new()))
    }

    fn target() -> Url {
        Url::parse("http://api.test/data").expect("valid target")
    }

    fn permissions_response(max_age: &str, methods: &str, headers: &str) -> FetchResponse {
        let mut response = FetchResponse::synthetic(target(), 200, None, Vec::new());
        response
            .headers
            .append_raw("access-control-allow-methods", methods);
        response
            .headers
            .append_raw("access-control-allow-headers", headers);
        response
            .headers
            .append_raw("access-control-max-age", max_age);
        response
    }

    fn allows(
        jar: &CookieJarRef,
        now_ms: u64,
        method: Method,
        headers: &[&str],
    ) -> bool {
        let headers: Vec<String> = headers.iter().map(|name| (*name).to_string()).collect();
        cache_allows(
            jar,
            now_ms,
            "http://page.test",
            &target(),
            false,
            method,
            &headers,
        )
    }

    #[test]
    fn max_age_parser_clamps_valid_values_that_exceed_u64() {
        assert_eq!(
            parse_max_age_seconds("999999999999999999999999999999999999999999"),
            Some(MAX_CORS_PREFLIGHT_MAX_AGE_SECS)
        );
        assert_eq!(parse_max_age_seconds("7201"), Some(7_200));
        assert_eq!(parse_max_age_seconds("7200"), Some(7_200));
    }

    #[test]
    fn max_age_parser_rejects_non_delta_seconds_syntax() {
        assert_eq!(parse_max_age_seconds(""), None);
        assert_eq!(parse_max_age_seconds("+60"), None);
        assert_eq!(parse_max_age_seconds("60s"), None);
        assert_eq!(parse_max_age_seconds("1, 2"), None);
    }

    #[test]
    fn malformed_max_age_uses_fetch_default() {
        let response = permissions_response("not-a-number", "PUT", "x-token");
        assert_eq!(max_age_seconds(&response), DEFAULT_CORS_PREFLIGHT_MAX_AGE_SECS);
    }

    #[test]
    fn later_preflight_preserves_unrelated_cached_permissions() {
        let jar = jar();
        store_permissions(
            &jar,
            0,
            "http://page.test",
            &target(),
            false,
            &permissions_response("60", "PUT", "x-token"),
        );
        store_permissions(
            &jar,
            100,
            "http://page.test",
            &target(),
            false,
            &permissions_response("60", "PATCH", "x-other"),
        );

        assert!(allows(&jar, 200, Method::Put, &["x-token"]));
        assert!(allows(&jar, 200, Method::Patch, &["x-other"]));
        assert!(!allows(&jar, 200, Method::Delete, &["x-token"]));
    }

    #[test]
    fn independently_cached_permissions_keep_independent_expiry() {
        let jar = jar();
        store_permissions(
            &jar,
            0,
            "http://page.test",
            &target(),
            false,
            &permissions_response("1", "PUT", "x-token"),
        );
        store_permissions(
            &jar,
            500,
            "http://page.test",
            &target(),
            false,
            &permissions_response("10", "PATCH", "x-other"),
        );

        assert!(!allows(&jar, 1_500, Method::Put, &["x-token"]));
        assert!(allows(&jar, 1_500, Method::Patch, &["x-other"]));
    }

    #[test]
    fn zero_max_age_expires_matching_permissions_without_erasing_others() {
        let jar = jar();
        store_permissions(
            &jar,
            0,
            "http://page.test",
            &target(),
            false,
            &permissions_response("60", "PUT, PATCH", "x-token, x-other"),
        );
        assert!(allows(&jar, 100, Method::Put, &["x-token"]));
        assert!(allows(&jar, 100, Method::Patch, &["x-other"]));

        store_permissions(
            &jar,
            100,
            "http://page.test",
            &target(),
            false,
            &permissions_response("0", "PUT", "x-token"),
        );

        assert!(!allows(&jar, 101, Method::Put, &["x-token"]));
        assert!(allows(&jar, 101, Method::Patch, &["x-other"]));
    }

    #[test]
    fn wildcard_header_permission_never_covers_authorization() {
        let jar = jar();
        store_permissions(
            &jar,
            0,
            "http://page.test",
            &target(),
            false,
            &permissions_response("60", "PUT", "*"),
        );

        assert!(allows(&jar, 1, Method::Put, &["x-token"]));
        assert!(!allows(&jar, 1, Method::Put, &["authorization"]));
    }
}
