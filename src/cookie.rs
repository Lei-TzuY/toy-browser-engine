// ============================================================
//  cookie.rs  —  RFC 6265 HTTP State Management & Cookie Jar
// ============================================================

use std::fmt;
use std::net::IpAddr;
use std::rc::Rc;

use crate::eventloop::Clock;
use crate::net::Url;

/// Maximum accepted `Set-Cookie` field-value size. Oversize state is rejected
/// rather than truncated so a server cannot make the browser allocate an
/// unbounded cookie from one response header.
pub const MAX_SET_COOKIE_BYTES: usize = 4096;
/// Per cookie-domain storage bound. When full, the oldest stored cookie in the
/// same domain is evicted before a new one is inserted.
pub const MAX_COOKIES_PER_DOMAIN: usize = 180;
/// Session-wide storage bound across all domains.
pub const MAX_COOKIES_TOTAL: usize = 3000;

/// SameSite policy for cookies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameSite {
    Strict,
    Lax,
    None,
}

/// A stored cookie with its attributes per RFC 6265.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    /// True when no `Domain` attribute was authored. Host-only cookies match
    /// exactly one host rather than leaking to its subdomains.
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
        if let Some(exp) = self.expires_at_ms {
            now_ms >= exp
        } else {
            false
        }
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
        // IPv4 and IPv6 literals as exact-only even for a Cookie that was
        // manually constructed rather than parsed through CookieJar.
        if request_host.parse::<IpAddr>().is_ok() || cookie_domain.parse::<IpAddr>().is_ok() {
            return request_host == cookie_domain;
        }

        domain_matches(&request_host, &cookie_domain)
    }

    /// Checks if this cookie matches the requested path.
    pub fn matches_path(&self, request_path: &str) -> bool {
        if self.path.is_empty() || self.path == "/" {
            return true;
        }
        if request_path == self.path {
            return true;
        }
        if request_path.starts_with(&self.path) {
            if self.path.ends_with('/') {
                return true;
            }
            if request_path.chars().nth(self.path.len()) == Some('/') {
                return true;
            }
        }
        false
    }
}

/// RFC 6265 Cookie Jar managing origin and path scoped cookies.
///
/// A standalone jar uses the `now_ms` arguments supplied to its methods. A
/// browser session can instead bind the jar to a shared [`Clock`] with
/// [`CookieJar::with_clock`]. Once bound, all expiry checks use that session
/// clock and ignore document-relative fallback timestamps. That distinction is
/// important because a new document resets JavaScript's event-loop time to
/// zero while cookie lifetime must continue monotonically across navigation.
///
/// `cookies` is kept in oldest-to-newest insertion order. Updating an existing
/// `(name, domain, path)` moves it to the end, so bounded eviction naturally
/// keeps recently refreshed state and removes the oldest state first.
pub struct CookieJar {
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
        // The clock is execution context, not stored cookie state. Two jars
        // with the same cookies compare equal regardless of which clock drives
        // their expiry checks.
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
    /// Standalone callers keep their explicit deterministic time; browser
    /// sessions use the bound monotonic clock so document time resets do not
    /// extend cookie lifetime.
    pub fn effective_now_ms(&self, fallback_ms: u64) -> u64 {
        self.clock
            .as_ref()
            .map(|clock| clock.now_ms().max(0.0) as u64)
            .unwrap_or(fallback_ms)
    }

    /// Parses a `Set-Cookie` header value into a [`Cookie`].
    ///
    /// Cookies are only defined for HTTP(S) URLs here. `Domain` attributes are
    /// accepted only when they domain-match the response host, preventing a
    /// server from planting cookies for an unrelated site. Modern secure-cookie
    /// invariants are enforced here too, before a cookie can enter the jar:
    /// `Secure` cannot be set over clear-text HTTP, `SameSite=None` requires
    /// `Secure`, and the `__Secure-` / `__Host-` prefixes keep their guarantees.
    ///
    /// Invalid control bytes, non-token names, invalid cookie-octet values and
    /// fields larger than [`MAX_SET_COOKIE_BYTES`] are rejected rather than
    /// normalized into a different cookie.
    pub fn parse_set_cookie(header: &str, url: &Url, now_ms: u64) -> Option<Cookie> {
        if !matches!(url.scheme(), "http" | "https") {
            return None;
        }
        if header.len() > MAX_SET_COOKIE_BYTES || has_unsafe_header_control(header) {
            return None;
        }

        let origin_host = url.host().trim_end_matches('.').to_ascii_lowercase();
        if origin_host.is_empty() {
            return None;
        }
        let secure_origin = url.scheme() == "https";
        let origin_is_ip = origin_host.parse::<IpAddr>().is_ok();

        let mut parts = header.split(';');
        let first = parts.next()?.trim();
        if first.is_empty() {
            return None;
        }

        let (name, value) = match first.find('=') {
            Some(idx) => (first[..idx].trim().to_string(), first[idx + 1..].trim().to_string()),
            None => (first.to_string(), String::new()),
        };

        if name.is_empty()
            || !name.bytes().all(is_cookie_name_byte)
            || !valid_cookie_value(&value)
        {
            return None;
        }

        let mut domain = origin_host.clone();
        let mut host_only = true;
        let default_path = {
            let p = url.path();
            if let Some(idx) = p.rfind('/') {
                if idx == 0 {
                    "/".to_string()
                } else {
                    p[..idx].to_string()
                }
            } else {
                "/".to_string()
            }
        };
        let mut path = default_path;
        let mut expires_at_ms: Option<u64> = None;
        let mut secure = false;
        let mut http_only = false;
        let mut same_site = SameSite::Lax;

        for part in parts {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let (attr_name, attr_val) = match part.find('=') {
                Some(idx) => (part[..idx].trim(), part[idx + 1..].trim()),
                None => (part, ""),
            };

            if attr_name.eq_ignore_ascii_case("Domain") && !attr_val.is_empty() {
                let raw = attr_val.trim();
                // A leading dot is legacy syntax and is ignored. A trailing
                // dot is not equivalent to the canonical host and must not be
                // normalized into a broader valid Domain attribute.
                if raw.ends_with('.') {
                    return None;
                }
                let candidate = raw.trim_start_matches('.').to_ascii_lowercase();
                if candidate.is_empty() || !domain_matches(&origin_host, &candidate) {
                    return None;
                }
                // Never interpret an IP literal as a DNS suffix. An explicit
                // Domain on an IP is only acceptable when it is exactly the
                // response host.
                if origin_is_ip && candidate != origin_host {
                    return None;
                }
                domain = candidate;
                host_only = false;
            } else if attr_name.eq_ignore_ascii_case("Path") && !attr_val.is_empty() {
                path = if attr_val.starts_with('/') {
                    attr_val.to_string()
                } else {
                    format!("/{}", attr_val)
                };
            } else if attr_name.eq_ignore_ascii_case("Max-Age") {
                if let Ok(secs) = attr_val.parse::<i64>() {
                    if secs <= 0 {
                        expires_at_ms = Some(0); // Immediately expired
                    } else {
                        expires_at_ms = Some(
                            now_ms.saturating_add((secs as u64).saturating_mul(1000)),
                        );
                    }
                }
            } else if attr_name.eq_ignore_ascii_case("Secure") {
                secure = true;
            } else if attr_name.eq_ignore_ascii_case("HttpOnly") {
                http_only = true;
            } else if attr_name.eq_ignore_ascii_case("SameSite") {
                if attr_val.eq_ignore_ascii_case("Strict") {
                    same_site = SameSite::Strict;
                } else if attr_val.eq_ignore_ascii_case("None") {
                    same_site = SameSite::None;
                } else {
                    same_site = SameSite::Lax;
                }
            }
        }

        // Secure cookies cannot be established over a non-secure channel.
        if secure && !secure_origin {
            return None;
        }
        // Modern browsers require SameSite=None cookies to be Secure.
        if same_site == SameSite::None && !secure {
            return None;
        }
        // Cookie name prefixes are case-sensitive by design.
        if name.starts_with("__Secure-") && (!secure || !secure_origin) {
            return None;
        }
        if name.starts_with("__Host-")
            && (!secure || !secure_origin || !host_only || path != "/")
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

    /// Stores or updates a cookie in the jar.
    ///
    /// Expired entries are purged on every mutation. A replacement is removed
    /// first and then appended, which refreshes its recency. New live state is
    /// bounded by [`MAX_COOKIES_PER_DOMAIN`] and [`MAX_COOKIES_TOTAL`]; the
    /// oldest eligible entry is evicted instead of allowing unbounded growth.
    pub fn store(&mut self, cookie: Cookie, now_ms: u64) {
        let now_ms = self.effective_now_ms(now_ms);
        self.cookies.retain(|c| !c.is_expired(now_ms));

        // Remove existing cookie with same (name, domain, path). Host-only and
        // Domain cookies with the same canonical triple replace one another.
        self.cookies.retain(|c| {
            !(c.name == cookie.name
                && c.domain.eq_ignore_ascii_case(&cookie.domain)
                && c.path == cookie.path)
        });

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
            let Some(oldest) = self
                .cookies
                .iter()
                .position(|stored| stored.domain.eq_ignore_ascii_case(&cookie.domain))
            else {
                break;
            };
            self.cookies.remove(oldest);
        }

        while self.cookies.len() >= MAX_COOKIES_TOTAL {
            self.cookies.remove(0);
        }

        self.cookies.push(cookie);
    }

    /// `document.cookie` getter: returns formatted string of matching non-HttpOnly cookies.
    pub fn get_document_cookie(&self, url: &Url, now_ms: u64) -> String {
        let now_ms = self.effective_now_ms(now_ms);
        let host = url.host();
        let path = url.path();
        let is_secure = url.scheme() == "https";

        let matching: Vec<String> = self
            .cookies
            .iter()
            .filter(|c| !c.is_expired(now_ms))
            .filter(|c| !c.http_only)
            .filter(|c| !c.secure || is_secure)
            .filter(|c| c.matches_domain(host))
            .filter(|c| c.matches_path(path))
            .map(|c| format!("{}={}", c.name, c.value))
            .collect();

        matching.join("; ")
    }

    /// `document.cookie = "key=value; ..."` setter from JavaScript.
    pub fn set_document_cookie(&mut self, cookie_str: &str, url: &Url, now_ms: u64) {
        let now_ms = self.effective_now_ms(now_ms);
        if let Some(cookie) = Self::parse_set_cookie(cookie_str, url, now_ms) {
            // Script cannot create an HttpOnly cookie.
            let mut cookie = cookie;
            cookie.http_only = false;
            self.store(cookie, now_ms);
        }
    }

    /// Formats the `Cookie:` header value for outgoing HTTP requests.
    pub fn get_http_cookie_header(&self, url: &Url, now_ms: u64) -> Option<String> {
        if !matches!(url.scheme(), "http" | "https") {
            return None;
        }
        let now_ms = self.effective_now_ms(now_ms);
        let host = url.host();
        let path = url.path();
        let is_secure = url.scheme() == "https";

        let matching: Vec<String> = self
            .cookies
            .iter()
            .filter(|c| !c.is_expired(now_ms))
            .filter(|c| !c.secure || is_secure)
            .filter(|c| c.matches_domain(host))
            .filter(|c| c.matches_path(path))
            .map(|c| format!("{}={}", c.name, c.value))
            .collect();

        if matching.is_empty() {
            None
        } else {
            Some(matching.join("; "))
        }
    }

    /// Number of stored, not-yet-purged cookies. Mutations purge expired state;
    /// read lookups also ignore expiration even before the next mutation.
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

fn has_unsafe_header_control(header: &str) -> bool {
    header.bytes().any(|byte| {
        byte == b'\r'
            || byte == b'\n'
            || byte == 0
            || byte == 0x7f
            || (byte < 0x20 && byte != b'\t')
    })
}

/// Cookie names use the RFC HTTP `token` character set.
fn is_cookie_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

/// Validate the RFC 6265 cookie-octet set. Quotes are permitted only as a
/// matching pair around a value; they remain in the stored value so outbound
/// `Cookie` serialization preserves what the server supplied.
fn valid_cookie_value(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.first() == Some(&b'"') || bytes.last() == Some(&b'"') {
        if bytes.len() < 2 || bytes.first() != Some(&b'"') || bytes.last() != Some(&b'"') {
            return false;
        }
        return bytes[1..bytes.len() - 1]
            .iter()
            .copied()
            .all(is_cookie_octet);
    }
    bytes.iter().copied().all(is_cookie_octet)
}

fn is_cookie_octet(byte: u8) -> bool {
    matches!(byte, 0x21 | 0x23..=0x2b | 0x2d..=0x3a | 0x3c..=0x5b | 0x5d..=0x7e)
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
