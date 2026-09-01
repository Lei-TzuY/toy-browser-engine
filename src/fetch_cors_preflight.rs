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

fn is_http_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || b"!#$%&'*+-.^_`|~".contains(&byte)
        })
}

fn comma_tokens(value: Option<String>) -> Result<Vec<String>, ()> {
    let mut tokens = Vec::new();
    for item in value.unwrap_or_default().split(',') {
        let token = item.trim();
        // HTTP list syntax permits empty list members around commas. Actual
        // Access-Control-Allow-* members still have to satisfy method/field-name,
        // both of which use the HTTP token grammar.
        if token.is_empty() {
            continue;
        }
        if !is_http_token(token) {
            return Err(());
        }
        tokens.push(token.to_string());
    }
    Ok(tokens)
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
    // This function is called after validate_preflight_response(). Keep the
    // cache fail-closed if an embedder calls it directly with malformed lists.
    let Ok(allowed_methods) = comma_tokens(response.headers.get("access-control-allow-methods")) else {
        return;
    };
    let Ok(allowed_headers) = comma_tokens(response.headers.get("access-control-allow-headers")) else {
        return;
    };
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

    let methods = comma_tokens(response.headers.get("access-control-allow-methods")).map_err(|_| {
        FetchError::Blocked("CORS: malformed Access-Control-Allow-Methods header".into())
    })?;
    let allowed_headers =
        comma_tokens(response.headers.get("access-control-allow-headers")).map_err(|_| {
            FetchError::Blocked("CORS: malformed Access-Control-Allow-Headers header".into())
        })?;

    if !is_cors_safelisted_method(requested_method) {
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
        let wildcard = !credentialed && allowed_headers.iter().any(|header| header == "*");
        for requested in requested_headers {
            let wildcard_allows = wildcard && !is_cors_non_wildcard_request_header_name(requested);
            if !wildcard_allows
                && !allowed_headers
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
    use super::{comma_tokens, is_http_token};

    #[test]
    fn http_token_accepts_method_and_field_name_characters() {
        for token in ["PUT", "x-token", "*", "x!#$%&'*+-.^_`|~"] {
            assert!(is_http_token(token), "expected valid token: {token:?}");
        }
    }

    #[test]
    fn http_token_rejects_separators_whitespace_and_controls() {
        for token in ["", "bad name", "bad/name", "\"quoted\"", "bad\tname", "bad\nname"] {
            assert!(!is_http_token(token), "expected invalid token: {token:?}");
        }
    }

    #[test]
    fn comma_token_lists_tolerate_empty_members_but_reject_invalid_members() {
        assert_eq!(
            comma_tokens(Some("PUT, ,PATCH,,x-token".into())).unwrap(),
            vec!["PUT", "PATCH", "x-token"]
        );
        assert!(comma_tokens(Some("PUT, bad method".into())).is_err());
        assert!(comma_tokens(Some("x-token, bad/name".into())).is_err());
    }
}
