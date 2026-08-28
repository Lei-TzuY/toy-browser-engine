// ============================================================
//  cookie.rs  —  RFC 6265 HTTP State Management & Cookie Jar
// ============================================================

use crate::net::Url;

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
    /// Absolute epoch timestamp in milliseconds when this cookie expires.
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CookieJar {
    cookies: Vec<Cookie>,
}

impl CookieJar {
    pub fn new() -> Self {
        CookieJar {
            cookies: Vec::new(),
        }
    }

    /// Parses a `Set-Cookie` header value into a [`Cookie`].
    ///
    /// Cookies are only defined for HTTP(S) URLs here. `Domain` attributes are
    /// accepted only when they domain-match the response host, preventing a
    /// server from planting cookies for an unrelated site.
    pub fn parse_set_cookie(header: &str, url: &Url, now_ms: u64) -> Option<Cookie> {
        if !matches!(url.scheme(), "http" | "https") {
            return None;
        }
        let origin_host = url.host().trim_end_matches('.').to_ascii_lowercase();
        if origin_host.is_empty() {
            return None;
        }

        let mut parts = header.split(';');
        let first = parts.next()?.trim();
        if first.is_empty() {
            return None;
        }

        let (name, value) = match first.find('=') {
            Some(idx) => (first[..idx].trim().to_string(), first[idx + 1..].trim().to_string()),
            None => (first.to_string(), String::new()),
        };

        if name.is_empty() {
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
                let candidate = attr_val
                    .trim_start_matches('.')
                    .trim_end_matches('.')
                    .to_ascii_lowercase();
                if candidate.is_empty() || !domain_matches(&origin_host, &candidate) {
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
                        expires_at_ms = Some(now_ms.saturating_add((secs as u64) * 1000));
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

    /// Stores or updates a cookie in the jar. If the cookie is expired, removes any existing match.
    pub fn store(&mut self, cookie: Cookie, now_ms: u64) {
        // Remove existing cookie with same (name, domain, path)
        self.cookies.retain(|c| {
            !(c.name == cookie.name
                && c.domain.eq_ignore_ascii_case(&cookie.domain)
                && c.path == cookie.path)
        });

        if !cookie.is_expired(now_ms) {
            self.cookies.push(cookie);
        }
    }

    /// `document.cookie` getter: returns formatted string of matching non-HttpOnly cookies.
    pub fn get_document_cookie(&self, url: &Url, now_ms: u64) -> String {
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

    /// Number of active cookies in the jar.
    pub fn len(&self) -> usize {
        self.cookies.len()
    }

    /// Check if the jar is empty.
    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }

    /// Clear all cookies.
    pub fn clear(&mut self) {
        self.cookies.clear();
    }
}

fn domain_matches(host: &str, domain: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let domain = domain
        .trim_start_matches('.')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host.is_empty() || domain.is_empty() {
        return false;
    }
    host == domain
        || (host.ends_with(&domain)
            && host
                .get(..host.len().saturating_sub(domain.len()))
                .is_some_and(|prefix| prefix.ends_with('.')))
}
