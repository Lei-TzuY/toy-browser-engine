// ============================================================
//  hsts.rs — HTTP Strict Transport Security policy/cache
// ============================================================

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;

use crate::net::Url;

/// Parsed `Strict-Transport-Security` policy from one valid response header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HstsPolicy {
    pub max_age_seconds: u64,
    pub include_subdomains: bool,
}

fn is_token_char(byte: u8) -> bool {
    byte.is_ascii()
        && byte > 0x20
        && byte != 0x7f
        && !matches!(
            byte,
            b'(' | b')' | b'<' | b'>' | b'@' | b',' | b';' | b':' | b'\\' | b'"' | b'/'
                | b'[' | b']' | b'?' | b'=' | b'{' | b'}'
        )
}

fn valid_token(input: &str) -> bool {
    !input.is_empty() && input.bytes().all(is_token_char)
}

fn split_directives(header: &str) -> Option<Vec<&str>> {
    let bytes = header.as_bytes();
    let mut directives = Vec::new();
    let mut start = 0;
    let mut in_quotes = false;
    let mut escaped = false;

    for (index, byte) in bytes.iter().copied().enumerate() {
        if in_quotes {
            if escaped {
                escaped = false;
                continue;
            }
            match byte {
                b'\\' => escaped = true,
                b'"' => in_quotes = false,
                b'\r' | b'\n' | 0x00..=0x08 | 0x0b | 0x0c | 0x0e..=0x1f | 0x7f => {
                    return None
                }
                _ => {}
            }
        } else {
            match byte {
                b'"' => in_quotes = true,
                b';' => {
                    directives.push(&header[start..index]);
                    start = index + 1;
                }
                b'\r' | b'\n' => return None,
                _ => {}
            }
        }
    }

    if in_quotes || escaped {
        return None;
    }
    directives.push(&header[start..]);
    Some(directives)
}

fn unescape_quoted_string(input: &str) -> Option<String> {
    let inner = input.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                let escaped = chars.next()?;
                if escaped == '\r' || escaped == '\n' || escaped.is_control() {
                    return None;
                }
                out.push(escaped);
            }
            '"' | '\r' | '\n' => return None,
            ch if ch.is_control() && ch != '\t' => return None,
            ch => out.push(ch),
        }
    }
    Some(out)
}

fn parse_directive_value(input: &str) -> Option<String> {
    if input.starts_with('"') {
        unescape_quoted_string(input)
    } else if valid_token(input) {
        Some(input.to_string())
    } else {
        None
    }
}

impl HstsPolicy {
    /// Parse the RFC 6797 directives needed by a user agent.
    ///
    /// `max-age` is mandatory and may be quoted. Directive names are
    /// case-insensitive; syntactically valid unknown extension directives are
    /// ignored. Repeating any directive or including malformed directive
    /// syntax invalidates the whole field, as required by RFC 6797.
    pub fn parse(header: &str) -> Option<HstsPolicy> {
        let mut max_age = None;
        let mut include_subdomains = false;
        let mut saw_include_subdomains = false;
        let mut seen_directives = HashSet::new();

        for raw in split_directives(header)? {
            let directive = raw.trim();
            if directive.is_empty() {
                continue;
            }
            let (name, value) = match directive.split_once('=') {
                Some((name, value)) => (name.trim(), Some(value.trim())),
                None => (directive, None),
            };

            if !valid_token(name) {
                return None;
            }
            let parsed_value = match value {
                Some(value) => Some(parse_directive_value(value)?),
                None => None,
            };

            if !seen_directives.insert(name.to_ascii_lowercase()) {
                return None;
            }

            if name.eq_ignore_ascii_case("max-age") {
                if max_age.is_some() {
                    return None;
                }
                let digits = parsed_value.as_deref()?;
                if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                max_age = digits.parse::<u64>().ok();
                if max_age.is_none() {
                    return None;
                }
            } else if name.eq_ignore_ascii_case("includesubdomains") {
                if saw_include_subdomains || parsed_value.is_some() {
                    return None;
                }
                saw_include_subdomains = true;
                include_subdomains = true;
            }
        }

        Some(HstsPolicy {
            max_age_seconds: max_age?,
            include_subdomains,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HstsEntry {
    expires_at_ms: u64,
    include_subdomains: bool,
}

/// In-memory Known HSTS Host cache for one browser session/profile.
///
/// Persistence and preload lists are intentionally separate concerns. This
/// cache models policies learned from secure HTTP responses and applies them
/// before a later HTTP load is dispatched.
#[derive(Debug, Clone, Default)]
pub struct HstsCache {
    entries: HashMap<String, HstsEntry>,
}

impl HstsCache {
    pub fn new() -> HstsCache {
        HstsCache::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Process one STS response field received for `response_url`.
    ///
    /// RFC 6797 requires insecure responses and IP-literal hosts to be ignored.
    /// A valid zero max-age removes only the congruent host's cached policy.
    pub fn observe_response(&mut self, response_url: &Url, header: &str, now_ms: u64) -> bool {
        if response_url.scheme() != "https" {
            return false;
        }
        let Some(host) = canonical_dns_host(response_url.host()) else {
            return false;
        };
        let Some(policy) = HstsPolicy::parse(header) else {
            return false;
        };
        self.purge_expired(now_ms);

        if policy.max_age_seconds == 0 {
            self.entries.remove(&host);
            return true;
        }

        let lifetime_ms = policy.max_age_seconds.saturating_mul(1000);
        self.entries.insert(
            host,
            HstsEntry {
                expires_at_ms: now_ms.saturating_add(lifetime_ms),
                include_subdomains: policy.include_subdomains,
            },
        );
        true
    }

    /// Remove expired learned state.
    pub fn purge_expired(&mut self, now_ms: u64) {
        self.entries
            .retain(|_, entry| entry.expires_at_ms > now_ms);
    }

    /// Whether `host` is a Known HSTS Host at `now_ms`.
    ///
    /// Exact matches apply regardless of `includeSubDomains`; superdomain
    /// matches apply only when that superdomain asserted the directive.
    pub fn is_known_host(&self, host: &str, now_ms: u64) -> bool {
        let Some(host) = canonical_dns_host(host) else {
            return false;
        };

        if self
            .entries
            .get(&host)
            .is_some_and(|entry| entry.expires_at_ms > now_ms)
        {
            return true;
        }

        let mut remainder = host.as_str();
        while let Some(dot) = remainder.find('.') {
            remainder = &remainder[dot + 1..];
            if self.entries.get(remainder).is_some_and(|entry| {
                entry.expires_at_ms > now_ms && entry.include_subdomains
            }) {
                return true;
            }
        }
        false
    }

    /// Upgrade an HTTP URL according to learned HSTS state.
    ///
    /// Explicit port 80 is rewritten to 443. Any other explicit port is
    /// preserved, and an absent port remains absent.
    pub fn upgrade_url(&self, url: &Url, now_ms: u64) -> Url {
        if url.scheme() != "http" || !self.is_known_host(url.host(), now_ms) {
            return url.clone();
        }

        let mut upgraded = url.clone();
        upgraded.set_scheme("https");
        if url.port() == Some(80) {
            upgraded.set_port(Some(443));
        }
        upgraded
    }
}

/// Canonicalize an ASCII DNS name for HSTS matching.
///
/// RFC 6797 defines Known HSTS Hosts in terms of domain names, not arbitrary
/// URI reg-name strings. The URL layer does not yet implement IDNA, so this
/// cache accepts ordinary ASCII DNS labels (including already-punycoded
/// `xn--...` labels) and deliberately rejects raw non-ASCII names rather than
/// creating a second, incompatible normalization scheme here.
///
/// One terminal root dot is ignored (`example.test.` == `example.test`), while
/// empty interior labels, illegal label characters, leading/trailing hyphens,
/// and DNS length violations are rejected. IP literals remain excluded by
/// HSTS itself.
fn canonical_dns_host(host: &str) -> Option<String> {
    let host = host.trim();
    if host.is_empty() {
        return None;
    }

    let ip_candidate = host.trim_start_matches('[').trim_end_matches(']');
    if ip_candidate.parse::<IpAddr>().is_ok() {
        return None;
    }

    let host = host.strip_suffix('.').unwrap_or(host);
    if host.is_empty() || host.len() > 253 || !host.is_ascii() {
        return None;
    }

    let mut canonical = String::with_capacity(host.len());
    for (index, label) in host.split('.').enumerate() {
        if label.is_empty() || label.len() > 63 {
            return None;
        }
        let bytes = label.as_bytes();
        if !bytes[0].is_ascii_alphanumeric()
            || !bytes[bytes.len() - 1].is_ascii_alphanumeric()
            || !bytes
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
        {
            return None;
        }
        if index != 0 {
            canonical.push('.');
        }
        canonical.extend(label.chars().map(|ch| ch.to_ascii_lowercase()));
    }
    Some(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(input: &str) -> Url {
        Url::parse(input).unwrap()
    }

    #[test]
    fn parses_standard_directives_and_quoted_age() {
        assert_eq!(
            HstsPolicy::parse("max-age=31536000; includeSubDomains"),
            Some(HstsPolicy {
                max_age_seconds: 31_536_000,
                include_subdomains: true,
            })
        );
        assert_eq!(
            HstsPolicy::parse("MAX-AGE=\"60\"; preload"),
            Some(HstsPolicy {
                max_age_seconds: 60,
                include_subdomains: false,
            })
        );
    }

    #[test]
    fn rejects_malformed_or_duplicated_standard_directives() {
        assert_eq!(HstsPolicy::parse("includeSubDomains"), None);
        assert_eq!(HstsPolicy::parse("max-age=abc"), None);
        assert_eq!(HstsPolicy::parse("max-age=1; max-age=2"), None);
        assert_eq!(
            HstsPolicy::parse("max-age=1; includeSubDomains; includeSubDomains"),
            None
        );
        assert_eq!(HstsPolicy::parse("max-age=1; includeSubDomains=yes"), None);
    }

    #[test]
    fn rejects_duplicate_extension_directives_case_insensitively() {
        assert_eq!(HstsPolicy::parse("max-age=60; preload; preload"), None);
        assert_eq!(HstsPolicy::parse("max-age=60; Foo=1; fOO=2"), None);
        assert_eq!(
            HstsPolicy::parse("max-age=60; preload; other=1"),
            Some(HstsPolicy {
                max_age_seconds: 60,
                include_subdomains: false,
            })
        );
    }

    #[test]
    fn validates_extension_directive_grammar_and_quoted_semicolons() {
        assert_eq!(
            HstsPolicy::parse("max-age=60; future=\"a;b\\\"c\"; flag"),
            Some(HstsPolicy {
                max_age_seconds: 60,
                include_subdomains: false,
            })
        );
        assert_eq!(HstsPolicy::parse("max-age=60; bad name=value"), None);
        assert_eq!(HstsPolicy::parse("max-age=60; future="), None);
        assert_eq!(HstsPolicy::parse("max-age=60; future=two words"), None);
        assert_eq!(HstsPolicy::parse("max-age=60; future=\"unterminated"), None);
        assert_eq!(HstsPolicy::parse("max-age=60; future=\"bad\rvalue\""), None);
    }

    #[test]
    fn secure_response_adds_and_zero_age_removes_policy() {
        let mut cache = HstsCache::new();
        let source = url("https://example.test/");
        assert!(cache.observe_response(&source, "max-age=10", 1_000));
        assert!(cache.is_known_host("example.test", 5_000));
        assert!(cache.observe_response(&source, "max-age=0; includeSubDomains", 6_000));
        assert!(!cache.is_known_host("example.test", 6_000));
    }

    #[test]
    fn insecure_and_ip_literal_sources_are_ignored() {
        let mut cache = HstsCache::new();
        assert!(!cache.observe_response(&url("http://example.test/"), "max-age=10", 0));
        assert!(!cache.observe_response(&url("https://127.0.0.1/"), "max-age=10", 0));
        assert!(!cache.observe_response(&url("https://[::1]/"), "max-age=10", 0));
        assert!(cache.is_empty());
    }

    #[test]
    fn canonicalizes_dns_case_and_one_terminal_root_dot() {
        assert_eq!(canonical_dns_host("Example.TEST."), Some("example.test".into()));
        assert_eq!(canonical_dns_host("XN--BCHER-KVA.Example"), Some("xn--bcher-kva.example".into()));
        assert_eq!(canonical_dns_host("example.test.."), None);
    }

    #[test]
    fn rejects_hosts_that_are_not_valid_ascii_dns_names() {
        assert_eq!(canonical_dns_host("bad..example"), None);
        assert_eq!(canonical_dns_host("-bad.example"), None);
        assert_eq!(canonical_dns_host("bad-.example"), None);
        assert_eq!(canonical_dns_host("bad_name.example"), None);
        assert_eq!(canonical_dns_host("bücher.example"), None);
        assert_eq!(canonical_dns_host(&format!("{}.example", "a".repeat(64))), None);
        assert_eq!(canonical_dns_host(&format!("{}.test", "a".repeat(249))), None);
    }

    #[test]
    fn include_subdomains_matches_only_on_label_boundaries() {
        let mut cache = HstsCache::new();
        assert!(cache.observe_response(
            &url("https://example.test/"),
            "max-age=100; includeSubDomains",
            0,
        ));
        assert!(cache.is_known_host("api.example.test", 1));
        assert!(cache.is_known_host("deep.api.example.test", 1));
        assert!(cache.is_known_host("API.EXAMPLE.TEST.", 1));
        assert!(!cache.is_known_host("notexample.test", 1));
        assert!(!cache.is_known_host("example.test.evil", 1));
        assert!(!cache.is_known_host("bad..api.example.test", 1));
    }

    #[test]
    fn child_policy_does_not_disable_parent_include_subdomains() {
        let mut cache = HstsCache::new();
        cache.observe_response(
            &url("https://example.test/"),
            "max-age=100; includeSubDomains",
            0,
        );
        cache.observe_response(&url("https://api.example.test/"), "max-age=0", 1);
        assert!(cache.is_known_host("api.example.test", 2));
    }

    #[test]
    fn expiry_is_monotonic_and_purgeable() {
        let mut cache = HstsCache::new();
        cache.observe_response(&url("https://example.test/"), "max-age=2", 10_000);
        assert!(cache.is_known_host("example.test", 11_999));
        assert!(!cache.is_known_host("example.test", 12_000));
        cache.purge_expired(12_000);
        assert!(cache.is_empty());
    }

    #[test]
    fn upgrade_preserves_url_and_applies_rfc_port_rules() {
        let mut cache = HstsCache::new();
        cache.observe_response(&url("https://example.test/"), "max-age=100", 0);

        assert_eq!(
            cache
                .upgrade_url(&url("http://example.test/a?q=1#frag"), 1)
                .to_string(),
            "https://example.test/a?q=1#frag"
        );
        assert_eq!(
            cache
                .upgrade_url(&url("http://example.test:80/a"), 1)
                .to_string(),
            "https://example.test:443/a"
        );
        assert_eq!(
            cache
                .upgrade_url(&url("http://example.test:8080/a"), 1)
                .to_string(),
            "https://example.test:8080/a"
        );
        assert_eq!(
            cache
                .upgrade_url(&url("https://example.test/a"), 1)
                .to_string(),
            "https://example.test/a"
        );
    }
}
