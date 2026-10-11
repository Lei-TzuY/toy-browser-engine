// ============================================================
//  cookie.rs  —  RFC 6265bis HTTP State Management & Cookie Jar
// ============================================================

use std::cmp::Reverse;
use std::fmt;
use std::net::IpAddr;
use std::rc::Rc;

use crate::eventloop::Clock;
use crate::net::Url;

/// RFC6265bis storage limit for the combined cookie name and value.
pub const MAX_COOKIE_NAME_VALUE_BYTES: usize = 4096;
/// RFC6265bis parsing limit for an individual cookie attribute value.
pub const MAX_COOKIE_ATTRIBUTE_VALUE_BYTES: usize = 1024;
/// This engine adopts the specification's recommended 400-day lifetime cap.
pub const MAX_COOKIE_AGE_SECONDS: u64 = 400 * 24 * 60 * 60;
/// Implementation-defined per-domain storage cap. The specification requires
/// general-purpose UAs to support at least 50; this engine keeps 180.
pub const MAX_COOKIES_PER_DOMAIN: usize = 180;
/// Session-wide cookie count cap, matching the specification's stated minimum
/// capability for general-purpose user agents.
pub const MAX_COOKIES_TOTAL: usize = 3000;

/// SameSite policy for cookies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameSite {
    Strict,
    Lax,
    None,
}

/// A stored cookie with its attributes per RFC 6265bis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    /// True when no effective `Domain` attribute was authored. Host-only
    /// cookies match exactly one host rather than its subdomains.
    pub host_only: bool,
    pub path: String,
    /// Absolute timestamp in the cookie jar's time domain when this expires.
    pub expires_at_ms: Option<u64>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: SameSite,
}

impl Cookie {
    pub fn is_expired(&self, now_ms: u64) -> bool {
        self.expires_at_ms.is_some_and(|exp| now_ms >= exp)
    }

    /// Checks if this cookie matches the requested domain.
    pub fn matches_domain(&self, host: &str) -> bool {
        let cookie_domain = self
            .domain
            .trim_start_matches('.')
            .trim_end_matches('.')
            .to_ascii_lowercase();
        let request_host = host.trim_end_matches('.').to_ascii_lowercase();

        if cookie_domain.is_empty() || request_host.is_empty() {
            return false;
        }
        if self.host_only {
            return request_host == cookie_domain;
        }

        // IP addresses are hosts, not registrable DNS suffixes. Treat both
        // IPv4 and IPv6 literals as exact-only even for manually-built Cookie
        // values that bypass the Set-Cookie parser.
        if request_host.parse::<IpAddr>().is_ok() || cookie_domain.parse::<IpAddr>().is_ok() {
            return request_host == cookie_domain;
        }

        domain_matches(&request_host, &cookie_domain)
    }

    /// Checks if this cookie matches the requested path.
    pub fn matches_path(&self, request_path: &str) -> bool {
        path_matches(request_path, &self.path)
    }
}

/// RFC 6265bis Cookie Jar managing origin and path scoped cookies.
///
/// A standalone jar uses the `now_ms` arguments supplied to its methods. A
/// browser session can instead bind the jar to a shared [`Clock`] with
/// [`CookieJar::with_clock`]. Once bound, expiry checks use that session clock
/// and ignore document-relative fallback timestamps, because navigation resets
/// JavaScript timer time but must not reset cookie lifetime.
pub struct CookieJar {
    /// Kept in creation order. Replacing a cookie updates the existing slot,
    /// preserving creation-order semantics used as the tie-breaker when
    /// serializing equal-length paths.
    cookies: Vec<Cookie>,
    clock: Option<Rc<dyn Clock>>,
}

impl fmt::Debug for CookieJar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CookieJar")
            .field("cookies", &self.cookies)
            .field("clock_bound", &self.clock.is_some())
            .finish()
    }
}

impl Clone for CookieJar {
    fn clone(&self) -> Self {
        CookieJar {
            cookies: self.cookies.clone(),
            clock: self.clock.clone(),
        }
    }
}

impl PartialEq for CookieJar {
    fn eq(&self, other: &Self) -> bool {
        // The clock is execution context, not stored cookie state.
        self.cookies == other.cookies
    }
}

impl Eq for CookieJar {}

impl Default for CookieJar {
    fn default() -> Self {
        Self::new()
    }
}

impl CookieJar {
    pub fn new() -> Self {
        CookieJar {
            cookies: Vec::new(),
            clock: None,
        }
    }

    /// Create a jar whose expiry time follows one shared browser/session clock.
    pub fn with_clock(clock: Rc<dyn Clock>) -> Self {
        CookieJar {
            cookies: Vec::new(),
            clock: Some(clock),
        }
    }

    /// Attach or replace the shared clock without changing stored cookies.
    pub fn bind_clock(&mut self, clock: Rc<dyn Clock>) {
        self.clock = Some(clock);
    }

    pub fn has_bound_clock(&self) -> bool {
        self.clock.is_some()
    }

    /// Resolve a caller-provided timestamp into the jar's actual time domain.
    pub fn effective_now_ms(&self, fallback_ms: u64) -> u64 {
        self.clock
            .as_ref()
            .map(|clock| clock.now_ms().max(0.0) as u64)
            .unwrap_or(fallback_ms)
    }

    /// Parse a Set-Cookie field-value using the permissive user-agent algorithm
    /// rather than the stricter server-generation grammar.
    ///
    /// In particular, UA parsing trims only SP/HTAB, allows characters that a
    /// conforming server would not generate, rejects CTLs (except HTAB), and
    /// applies the 4096-byte bound to *name + value*, not to the entire field.
    /// Attribute values over 1024 bytes are ignored individually.
    pub fn parse_set_cookie(header: &str, url: &Url, now_ms: u64) -> Option<Cookie> {
        if !matches!(url.scheme(), "http" | "https") || has_forbidden_cookie_ctl(header) {
            return None;
        }

        let origin_host = url.host().trim_end_matches('.').to_ascii_lowercase();
        if origin_host.is_empty() {
            return None;
        }
        let secure_origin = url.scheme() == "https";
        let origin_is_ip = origin_host.parse::<IpAddr>().is_ok();

        let mut parts = header.split(';');
        let pair = trim_wsp(parts.next()?);
        let (name, value) = match pair.find('=') {
            Some(index) => (
                trim_wsp(&pair[..index]).to_string(),
                trim_wsp(&pair[index + 1..]).to_string(),
            ),
            None => (String::new(), trim_wsp(pair).to_string()),
        };

        if name.len().saturating_add(value.len()) > MAX_COOKIE_NAME_VALUE_BYTES
            || (name.is_empty() && value.is_empty())
        {
            return None;
        }

        let default_path = default_cookie_path(url.path());
        let mut domain = origin_host.clone();
        let mut host_only = true;
        let mut path = default_path.clone();
        let mut path_attribute_present = false;
        let mut expires_at_ms: Option<u64> = None;
        let mut secure = false;
        let mut http_only = false;
        let mut same_site = SameSite::Lax;

        for raw_part in parts {
            let part = trim_wsp(raw_part);
            if part.is_empty() {
                continue;
            }
            let (attr_name, attr_val) = match part.find('=') {
                Some(index) => (trim_wsp(&part[..index]), trim_wsp(&part[index + 1..])),
                None => (trim_wsp(part), ""),
            };

            if attr_val.len() > MAX_COOKIE_ATTRIBUTE_VALUE_BYTES {
                continue;
            }

            if attr_name.eq_ignore_ascii_case("Domain") {
                let candidate = attr_val
                    .strip_prefix('.')
                    .unwrap_or(attr_val)
                    .to_ascii_lowercase();

                if candidate.is_empty() {
                    continue;
                }
                if !candidate.is_ascii() || !domain_matches(&origin_host, &candidate) {
                    return None;
                }
                if origin_is_ip && candidate != origin_host {
                    return None;
                }
                domain = candidate;
                host_only = false;
            } else if attr_name.eq_ignore_ascii_case("Path") {
                path_attribute_present = true;
                path = if attr_val.starts_with('/') {
                    attr_val.to_string()
                } else {
                    default_path.clone()
                };
            } else if attr_name.eq_ignore_ascii_case("Max-Age") {
                if let Some(seconds) = parse_max_age(attr_val) {
                    expires_at_ms = if seconds <= 0 {
                        Some(0)
                    } else {
                        let capped = (seconds as u64).min(MAX_COOKIE_AGE_SECONDS);
                        Some(now_ms.saturating_add(capped.saturating_mul(1000)))
                    };
                }
            } else if attr_name.eq_ignore_ascii_case("Secure") {
                secure = true;
            } else if attr_name.eq_ignore_ascii_case("HttpOnly") {
                http_only = true;
            } else if attr_name.eq_ignore_ascii_case("SameSite") {
                same_site = if attr_val.eq_ignore_ascii_case("None") {
                    SameSite::None
                } else if attr_val.eq_ignore_ascii_case("Strict") {
                    SameSite::Strict
                } else {
                    SameSite::Lax
                };
            }
        }

        if secure && !secure_origin {
            return None;
        }
        if same_site == SameSite::None && !secure {
            return None;
        }

        if starts_with_ascii_case_insensitive(&name, "__Secure-") && !secure {
            return None;
        }
        if starts_with_ascii_case_insensitive(&name, "__Host-")
            && (!secure || !host_only || !path_attribute_present || path != "/")
        {
            return None;
        }
        if name.is_empty()
            && (starts_with_ascii_case_insensitive(&value, "__Secure-")
                || starts_with_ascii_case_insensitive(&value, "__Host-"))
        {
            return None;
        }

        Some(Cookie {
            name,
            value,
            domain,
            host_only,
            path,
            expires_at_ms,
            secure,
            http_only,
            same_site,
        })
    }

    /// Parse and store one Set-Cookie value received from HTTP transport.
    /// Returns false when parsing or RFC secure-cookie integrity policy rejects
    /// the state. Callers that already parsed the Cookie can use
    /// [`CookieJar::store_from_http`] directly.
    pub fn store_set_cookie(&mut self, header: &str, source_url: &Url, now_ms: u64) -> bool {
        let now_ms = self.effective_now_ms(now_ms);
        let Some(cookie) = Self::parse_set_cookie(header, source_url, now_ms) else {
            return false;
        };
        self.store_from_http(cookie, source_url, now_ms)
    }

    /// Store an HTTP-received cookie with request-URI context.
    ///
    /// RFC6265bis prevents an insecure origin from overlaying a Secure cookie
    /// with a non-Secure cookie of the same name when their domains overlap and
    /// the new cookie path path-matches the existing Secure cookie path. The
    /// path comparison is deliberately non-symmetric.
    pub fn store_from_http(&mut self, cookie: Cookie, source_url: &Url, now_ms: u64) -> bool {
        if !matches!(source_url.scheme(), "http" | "https") {
            return false;
        }
        let now_ms = self.effective_now_ms(now_ms);
        let source_is_secure = source_url.scheme() == "https";

        // Defensive check for callers of this lower-level method that did not
        // obtain `cookie` from parse_set_cookie.
        if cookie.secure && !source_is_secure {
            return false;
        }
        if !source_is_secure && !cookie.secure && self.would_overlay_secure(&cookie, now_ms) {
            return false;
        }

        self.store(cookie, now_ms);
        true
    }

    /// Low-level storage primitive for an already-accepted Cookie.
    ///
    /// This method intentionally has no source-URL policy. Network and
    /// document-cookie paths should use `store_from_http` / `set_document_cookie`;
    /// tests and trusted embedders may use this primitive to seed a jar.
    pub fn store(&mut self, cookie: Cookie, now_ms: u64) {
        let now_ms = self.effective_now_ms(now_ms);
        self.cookies.retain(|stored| !stored.is_expired(now_ms));

        if let Some(position) = self.cookies.iter().position(|stored| {
            stored.name == cookie.name
                && stored.domain.eq_ignore_ascii_case(&cookie.domain)
                && stored.host_only == cookie.host_only
                && stored.path == cookie.path
        }) {
            if cookie.is_expired(now_ms) {
                self.cookies.remove(position);
            } else {
                self.cookies[position] = cookie;
            }
            return;
        }

        if cookie.is_expired(now_ms) {
            return;
        }

        while self
            .cookies
            .iter()
            .filter(|stored| stored.domain.eq_ignore_ascii_case(&cookie.domain))
            .count()
            >= MAX_COOKIES_PER_DOMAIN
        {
            let oldest = self
                .cookies
                .iter()
                .position(|stored| {
                    stored.domain.eq_ignore_ascii_case(&cookie.domain) && !stored.secure
                })
                .or_else(|| {
                    self.cookies
                        .iter()
                        .position(|stored| stored.domain.eq_ignore_ascii_case(&cookie.domain))
                });
            let Some(oldest) = oldest else { break };
            self.cookies.remove(oldest);
        }

        while self.cookies.len() >= MAX_COOKIES_TOTAL {
            self.cookies.remove(0);
        }

        self.cookies.push(cookie);
    }

    /// `document.cookie` getter: matching non-HttpOnly cookies in cookie-string
    /// order (longer paths first, creation order for equal path lengths).
    pub fn get_document_cookie(&self, url: &Url, now_ms: u64) -> String {
        self.cookie_string(url, now_ms, false)
    }

    /// `document.cookie = "..."` setter from JavaScript / a non-HTTP API.
    ///
    /// A script cannot create HttpOnly state, cannot overwrite an existing
    /// HttpOnly cookie with the same RFC identity, and an insecure document
    /// cannot overlay existing Secure state.
    pub fn set_document_cookie(&mut self, cookie_str: &str, url: &Url, now_ms: u64) {
        let now_ms = self.effective_now_ms(now_ms);
        let Some(cookie) = Self::parse_set_cookie(cookie_str, url, now_ms) else {
            return;
        };
        if cookie.http_only || self.has_http_only_collision(&cookie, now_ms) {
            return;
        }
        if url.scheme() != "https" && !cookie.secure && self.would_overlay_secure(&cookie, now_ms) {
            return;
        }
        self.store(cookie, now_ms);
    }

    /// Formats the `Cookie:` header value for outgoing HTTP requests.
    pub fn get_http_cookie_header(&self, url: &Url, now_ms: u64) -> Option<String> {
        if !matches!(url.scheme(), "http" | "https") {
            return None;
        }
        let value = self.cookie_string(url, now_ms, true);
        (!value.is_empty()).then_some(value)
    }

    fn cookie_string(&self, url: &Url, now_ms: u64, include_http_only: bool) -> String {
        if !matches!(url.scheme(), "http" | "https") {
            return String::new();
        }
        let now_ms = self.effective_now_ms(now_ms);
        let host = url.host();
        let path = url.path();
        let is_secure = url.scheme() == "https";

        let mut matching: Vec<&Cookie> = self
            .cookies
            .iter()
            .filter(|cookie| !cookie.is_expired(now_ms))
            .filter(|cookie| include_http_only || !cookie.http_only)
            .filter(|cookie| !cookie.secure || is_secure)
            .filter(|cookie| cookie.matches_domain(host))
            .filter(|cookie| cookie.matches_path(path))
            .collect();

        matching.sort_by_key(|cookie| Reverse(cookie.path.len()));
        matching
            .into_iter()
            .map(serialize_cookie_pair)
            .collect::<Vec<_>>()
            .join("; ")
    }

    fn has_http_only_collision(&self, cookie: &Cookie, now_ms: u64) -> bool {
        self.cookies.iter().any(|stored| {
            !stored.is_expired(now_ms)
                && stored.http_only
                && stored.name == cookie.name
                && stored.domain.eq_ignore_ascii_case(&cookie.domain)
                && stored.host_only == cookie.host_only
                && stored.path == cookie.path
        })
    }

    fn would_overlay_secure(&self, cookie: &Cookie, now_ms: u64) -> bool {
        self.cookies.iter().any(|stored| {
            !stored.is_expired(now_ms)
                && stored.secure
                && stored.name == cookie.name
                && domains_overlap(&stored.domain, &cookie.domain)
                // Non-symmetric by specification: the new cookie path must
                // path-match the existing Secure cookie path.
                && path_matches(&cookie.path, &stored.path)
        })
    }

    /// Number of stored, not-yet-purged cookies. Mutations purge expired state;
    /// retrievals ignore expired entries even before the next mutation.
    pub fn len(&self) -> usize {
        self.cookies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }

    pub fn clear(&mut self) {
        self.cookies.clear();
    }
}

fn trim_wsp(value: &str) -> &str {
    value.trim_matches(|character| character == ' ' || character == '\t')
}

fn has_forbidden_cookie_ctl(value: &str) -> bool {
    value
        .bytes()
        .any(|byte| matches!(byte, 0x00..=0x08 | 0x0a..=0x1f | 0x7f))
}

fn parse_max_age(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    let digits = if bytes[0] == b'-' {
        if bytes.len() == 1 {
            return None;
        }
        &bytes[1..]
    } else {
        bytes
    };
    if !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    value.parse::<i64>().ok()
}

fn default_cookie_path(request_path: &str) -> String {
    if !request_path.starts_with('/') || request_path.matches('/').count() <= 1 {
        return "/".to_string();
    }
    match request_path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(index) => request_path[..index].to_string(),
    }
}

fn path_matches(request_path: &str, cookie_path: &str) -> bool {
    if request_path == cookie_path {
        return true;
    }
    if !request_path.starts_with(cookie_path) {
        return false;
    }
    cookie_path.ends_with('/')
        || request_path
            .as_bytes()
            .get(cookie_path.len())
            .is_some_and(|byte| *byte == b'/')
}

fn serialize_cookie_pair(cookie: &Cookie) -> String {
    match (cookie.name.is_empty(), cookie.value.is_empty()) {
        (false, false) => format!("{}={}", cookie.name, cookie.value),
        (false, true) => format!("{}=", cookie.name),
        (true, false) => cookie.value.clone(),
        (true, true) => String::new(),
    }
}

fn starts_with_ascii_case_insensitive(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

fn domains_overlap(first: &str, second: &str) -> bool {
    domain_matches(first, second) || domain_matches(second, first)
}

fn domain_matches(host: &str, domain: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let domain = domain.trim_start_matches('.').to_ascii_lowercase();
    if host.is_empty() || domain.is_empty() {
        return false;
    }

    if host.parse::<IpAddr>().is_ok() || domain.parse::<IpAddr>().is_ok() {
        return host == domain;
    }

    host == domain
        || (host.ends_with(&domain)
            && host
                .get(..host.len().saturating_sub(domain.len()))
                .is_some_and(|prefix| prefix.ends_with('.')))
}
