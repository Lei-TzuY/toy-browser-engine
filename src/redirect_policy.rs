//! Redirect request planning above the raw HTTP transport.
//!
//! `net::http::send_once` intentionally exposes one response at a time.  This
//! module owns the browser-facing part of following a redirect: resolving
//! `Location`, applying Fetch method/body rewriting rules, removing credentials
//! that were selected for the previous hop, and enforcing a redirect budget.
//! It performs no I/O, so Cookie/HSTS/session policy can process each response
//! and then dispatch the returned request as the next independent hop.

use std::fmt;

use crate::net::{FetchRequest, FetchResponse, Method, Url};

/// Error produced while constructing a redirect hop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedirectError {
    /// `Location` could not be resolved against the current request URL.
    InvalidLocation(String),
    /// Following this response would exceed the configured redirect budget.
    TooManyRedirects(String),
}

impl fmt::Display for RedirectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RedirectError::InvalidLocation(location) => {
                write!(f, "invalid redirect location: {location}")
            }
            RedirectError::TooManyRedirects(url) => write!(f, "too many redirects: {url}"),
        }
    }
}

impl std::error::Error for RedirectError {}

/// Stateful planner for one redirect chain.
///
/// The planner deliberately does not preserve `Cookie` across hops. Cookie
/// selection depends on the destination URL's Domain/Path/Secure/SameSite
/// context and must be re-run by the browser policy layer before dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedirectPlanner {
    max_redirects: u8,
    followed: u8,
}

impl RedirectPlanner {
    pub fn new(max_redirects: u8) -> Self {
        Self {
            max_redirects,
            followed: 0,
        }
    }

    /// Number of redirects already accepted in this chain.
    pub fn followed(&self) -> u8 {
        self.followed
    }

    /// Return the request for the next hop, or `None` when `response` is not a
    /// Fetch redirect response with a `Location` header.
    ///
    /// This mutates the redirect count only after a valid next URL has been
    /// constructed, so malformed `Location` values do not consume the budget.
    pub fn next_request(
        &mut self,
        request: &FetchRequest,
        response: &FetchResponse,
    ) -> Result<Option<FetchRequest>, RedirectError> {
        if !is_redirect_status(response.status) {
            return Ok(None);
        }
        let Some(location) = response.headers.get("location") else {
            return Ok(None);
        };

        if self.followed >= self.max_redirects {
            return Err(RedirectError::TooManyRedirects(request.url.to_string()));
        }

        let mut next_url = request
            .url
            .join(&location)
            .map_err(|_| RedirectError::InvalidLocation(location.clone()))?;

        // Fetch's "location URL" algorithm carries the current request
        // fragment forward when Location itself does not provide one.  A
        // redirect may replace it explicitly, including with an empty `#`.
        if !location_has_fragment(&location) {
            next_url.set_fragment(request.url.fragment().map(str::to_string));
        }

        let mut headers = request.headers.clone();

        // This Cookie value was selected for `request.url`. Even a same-origin
        // path redirect can change Path eligibility, so force the session layer
        // to derive a fresh value for every hop.
        headers.delete("cookie");

        // Origin-bound credentials must not be forwarded merely because a
        // server supplied a cross-origin Location.
        if !same_origin(&request.url, &next_url) {
            headers.delete("authorization");
            headers.delete("proxy-authorization");
        }

        let rewrite_to_get = redirects_to_get(response.status, request.method);
        let method = if rewrite_to_get {
            Method::Get
        } else {
            request.method
        };
        let body = if rewrite_to_get {
            remove_request_body_headers(&mut headers);
            None
        } else {
            request.body.clone()
        };

        self.followed += 1;
        Ok(Some(FetchRequest::new(next_url, method, headers, body)))
    }
}

/// Status codes Fetch treats as redirects.
pub fn is_redirect_status(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// Whether this redirect changes the next request to GET.
pub fn redirects_to_get(status: u16, method: Method) -> bool {
    matches!(status, 301 | 302) && method == Method::Post
        || status == 303 && !matches!(method, Method::Get | Method::Head)
}

fn location_has_fragment(location: &str) -> bool {
    location.trim().contains('#')
}

fn remove_request_body_headers(headers: &mut crate::net::HeaderMap) {
    for name in [
        "content-encoding",
        "content-language",
        "content-location",
        "content-type",
    ] {
        headers.delete(name);
    }
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host().eq_ignore_ascii_case(right.host())
        && left.port_or_default() == right.port_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{FetchResponse, HeaderMap};

    fn request(url: &str, method: Method) -> FetchRequest {
        let mut headers = HeaderMap::new();
        headers.insert_raw("cookie", "sid=old");
        headers.insert_raw("authorization", "Bearer secret");
        headers.insert_raw("proxy-authorization", "Basic proxy-secret");
        headers.insert_raw("content-type", "text/plain");
        headers.insert_raw("x-keep", "yes");
        FetchRequest::new(
            Url::parse(url).unwrap(),
            method,
            headers,
            Some(b"payload".to_vec()),
        )
    }

    fn response(url: &str, status: u16, location: Option<&str>) -> FetchResponse {
        let mut response = FetchResponse::synthetic(
            Url::parse(url).unwrap(),
            status,
            None,
            Vec::new(),
        );
        if let Some(location) = location {
            response.headers.insert_raw("location", location);
        }
        response
    }

    #[test]
    fn post_302_becomes_get_and_drops_body_metadata() {
        let original = request("http://example.test/start", Method::Post);
        let mut planner = RedirectPlanner::new(5);
        let next = planner
            .next_request(
                &original,
                &response("http://example.test/start", 302, Some("/next")),
            )
            .unwrap()
            .unwrap();

        assert_eq!(next.url.to_string(), "http://example.test/next");
        assert_eq!(next.method, Method::Get);
        assert_eq!(next.body, None);
        assert!(!next.headers.has("content-type"));
        assert!(!next.headers.has("cookie"));
        assert!(next.headers.has("authorization"));
        assert_eq!(next.headers.get("x-keep").as_deref(), Some("yes"));
    }

    #[test]
    fn cross_origin_307_preserves_body_but_strips_origin_credentials() {
        let original = request("http://example.test/start", Method::Put);
        let mut planner = RedirectPlanner::new(5);
        let next = planner
            .next_request(
                &original,
                &response(
                    "http://example.test/start",
                    307,
                    Some("http://other.test/upload"),
                ),
            )
            .unwrap()
            .unwrap();

        assert_eq!(next.method, Method::Put);
        assert_eq!(next.body.as_deref(), Some(b"payload".as_slice()));
        assert!(next.headers.has("content-type"));
        assert!(!next.headers.has("cookie"));
        assert!(!next.headers.has("authorization"));
        assert!(!next.headers.has("proxy-authorization"));
    }

    #[test]
    fn redirect_budget_is_enforced_exactly() {
        let original = FetchRequest::get(Url::parse("http://example.test/a").unwrap());
        let redirect = response("http://example.test/a", 301, Some("/b"));
        let mut planner = RedirectPlanner::new(1);
        let next = planner.next_request(&original, &redirect).unwrap().unwrap();
        assert_eq!(planner.followed(), 1);
        assert_eq!(
            planner.next_request(&next, &redirect),
            Err(RedirectError::TooManyRedirects(next.url.to_string()))
        );
    }

    #[test]
    fn non_redirect_or_missing_location_does_not_consume_budget() {
        let original = FetchRequest::get(Url::parse("http://example.test/a").unwrap());
        let mut planner = RedirectPlanner::new(0);
        assert_eq!(
            planner.next_request(&original, &response("http://example.test/a", 304, Some("/b"))),
            Ok(None)
        );
        assert_eq!(
            planner.next_request(&original, &response("http://example.test/a", 302, None)),
            Ok(None)
        );
        assert_eq!(planner.followed(), 0);
    }

    #[test]
    fn relative_location_is_resolved_against_the_current_request_url() {
        let original = FetchRequest::get(Url::parse("http://example.test/a/b/page").unwrap());
        let mut planner = RedirectPlanner::new(5);
        let next = planner
            .next_request(
                &original,
                &response("http://example.test/a/b/page", 308, Some("../target?q=1")),
            )
            .unwrap()
            .unwrap();
        assert_eq!(next.url.to_string(), "http://example.test/a/target?q=1");
    }

    #[test]
    fn location_without_fragment_inherits_current_request_fragment() {
        let original = FetchRequest::get(
            Url::parse("https://example.test/start#section-2").unwrap(),
        );
        let mut planner = RedirectPlanner::new(1);
        let next = planner
            .next_request(
                &original,
                &response("https://example.test/start", 302, Some("/next?q=1")),
            )
            .unwrap()
            .unwrap();

        assert_eq!(
            next.url.to_string(),
            "https://example.test/next?q=1#section-2"
        );
    }

    #[test]
    fn explicit_location_fragment_overrides_inherited_fragment() {
        let original = FetchRequest::get(
            Url::parse("https://example.test/start#old").unwrap(),
        );
        let mut planner = RedirectPlanner::new(1);
        let next = planner
            .next_request(
                &original,
                &response("https://example.test/start", 302, Some("/next#new")),
            )
            .unwrap()
            .unwrap();

        assert_eq!(next.url.to_string(), "https://example.test/next#new");
    }
}
