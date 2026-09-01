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

#[derive(Debug, Clone)]
struct CorsPreflightCacheEntry {
    source_origin: String,
    target_url: String,
    credentialed: bool,
    allowed_methods: Vec<String>,
    allowed_headers: Vec<String>,
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

fn max_age_seconds(response: &FetchResponse) -> u64 {
    response
        .headers
        .get("access-control-max-age")
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_CORS_PREFLIGHT_MAX_AGE_SECS)
        .min(MAX_CORS_PREFLIGHT_MAX_AGE_SECS)
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
        entries.iter().any(|entry| {
            if entry.source_origin != source_origin
                || entry.target_url != target_url
                || (!entry.credentialed && credentialed)
            {
                return false;
            }

            let method_allowed = is_cors_safelisted_method(requested_method)
                || entry.allowed_methods.iter().any(|method| {
                    method == requested_method.as_str() || (method == "*" && !credentialed)
                });
            method_allowed
                && requested_headers.iter().all(|requested| {
                    entry.allowed_headers.iter().any(|allowed| {
                        allowed.eq_ignore_ascii_case(requested)
                            || (allowed == "*"
                                && !credentialed
                                && !is_cors_non_wildcard_request_header_name(requested))
                    })
                })
        })
    })
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
    if max_age == 0 {
        return;
    }

    let now_ms = effective_now_ms(jar, now_ms);
    let target_url = target_cache_key(target);
    let expires_at_ms = now_ms.saturating_add(max_age.saturating_mul(1_000));
    let allowed_methods = comma_tokens(response.headers.get("access-control-allow-methods"));
    let allowed_headers = comma_tokens(response.headers.get("access-control-allow-headers"));
    let source_origin = source_origin.to_string();

    with_session_cache(jar, |entries| {
        entries.retain(|entry| {
            entry.expires_at_ms > now_ms
                && (entry.source_origin != source_origin
                    || entry.target_url != target_url
                    || entry.credentialed != credentialed)
        });
        if entries.len() >= MAX_CORS_PREFLIGHT_CACHE_ENTRIES_PER_SESSION {
            entries.remove(0);
        }
        entries.push(CorsPreflightCacheEntry {
            source_origin,
            target_url,
            credentialed,
            allowed_methods,
            allowed_headers,
            expires_at_ms,
        });
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
