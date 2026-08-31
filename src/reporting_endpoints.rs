//! Reporting API endpoint parsing and Integrity-Policy report resolution.
//!
//! `Integrity-Policy` names Reporting API endpoints, while the navigation
//! response's `Reporting-Endpoints` field maps those names to concrete secure
//! destinations. Keeping that mapping separate from policy evaluation avoids
//! turning an unknown or malformed endpoint name into an accidental network
//! destination.

use std::collections::HashSet;

use crate::integrity_policy_reporting::IntegrityViolationReport;
use crate::net::{FetchResponse, HeaderMap, Url};

pub const REPORTING_ENDPOINTS_HEADER: &str = "reporting-endpoints";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportingEndpoint {
    pub name: String,
    pub url: Url,
}

/// Response-committed Reporting API endpoint mapping.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReportingEndpoints {
    endpoints: Vec<ReportingEndpoint>,
}

impl ReportingEndpoints {
    /// Parse `Reporting-Endpoints` without a response URL.
    ///
    /// This compatibility entry point accepts only absolute endpoint URLs. Use
    /// [`ReportingEndpoints::from_response`] when processing a real response;
    /// the Reporting API defines endpoint strings as URI-references and resolves
    /// them against that response's URL.
    pub fn from_headers(headers: &HeaderMap) -> Self {
        let Some(value) = headers.get(REPORTING_ENDPOINTS_HEADER) else {
            return Self::default();
        };
        Self::parse(&value)
    }

    /// Process `Reporting-Endpoints` in the context of the response that
    /// declared it.
    ///
    /// Relative URI-references are resolved against `response.url`. An
    /// untrustworthy response cannot establish reporting endpoints at all.
    pub fn from_response(response: &FetchResponse) -> Self {
        if !is_potentially_trustworthy(&response.url) {
            return Self::default();
        }
        let Some(value) = response.headers.get(REPORTING_ENDPOINTS_HEADER) else {
            return Self::default();
        };
        Self::parse_with_base(&value, &response.url)
    }

    /// Parse an endpoint dictionary where endpoint values must be absolute.
    pub fn parse(value: &str) -> Self {
        Self::parse_impl(value, None)
    }

    /// Parse an endpoint dictionary and resolve every URI-reference against
    /// `base_url`, matching Reporting API response processing.
    pub fn parse_with_base(value: &str, base_url: &Url) -> Self {
        Self::parse_impl(value, Some(base_url))
    }

    fn parse_impl(value: &str, base_url: Option<&Url>) -> Self {
        let Some(members) = split_dictionary_members(value) else {
            return Self::default();
        };
        let mut names = HashSet::new();
        let mut endpoints = Vec::new();

        for member in members {
            let Some((raw_name, raw_value)) = member.split_once('=') else {
                return Self::default();
            };
            let name = raw_name.trim();
            if !valid_dictionary_key(name) || !names.insert(name.to_string()) {
                return Self::default();
            }

            // Reporting-Endpoints is a Structured Fields dictionary whose
            // member value must be a String. Parameters belong to the member
            // and do not change the endpoint URI-reference, so parse and
            // validate them while intentionally ignoring their values here.
            let Some(url_text) = parse_string_item_with_parameters(raw_value.trim()) else {
                return Self::default();
            };
            let resolved = resolve_uri_reference(&url_text, base_url);
            let Some(mut url) = resolved else {
                // The dictionary itself was valid, but this member was not a
                // usable URI-reference. Reporting ignores that member only.
                continue;
            };

            // Reporting destinations are network sinks and must be potentially
            // trustworthy. In this engine that means HTTPS, plus loopback HTTP
            // development origins.
            if !is_potentially_trustworthy(&url) {
                continue;
            }
            url.set_fragment(None);
            endpoints.push(ReportingEndpoint {
                name: name.to_string(),
                url,
            });
        }

        Self { endpoints }
    }

    pub fn get(&self, name: &str) -> Option<&Url> {
        self.endpoints
            .iter()
            .find(|endpoint| endpoint.name == name)
            .map(|endpoint| &endpoint.url)
    }

    pub fn iter(&self) -> impl Iterator<Item = &ReportingEndpoint> {
        self.endpoints.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.endpoints.is_empty()
    }

    pub fn len(&self) -> usize {
        self.endpoints.len()
    }
}

/// Integrity violation paired with the concrete Reporting API destination that
/// may receive it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedIntegrityViolationReport {
    pub endpoint_name: String,
    pub endpoint_url: Url,
    pub report: IntegrityViolationReport,
}

/// Resolve queued/generated Integrity-Policy reports against the response's
/// Reporting-Endpoints mapping. Reports naming missing, insecure, or malformed
/// endpoints are intentionally dropped from delivery.
pub fn resolve_integrity_violation_reports(
    reports: &[IntegrityViolationReport],
    endpoints: &ReportingEndpoints,
) -> Vec<ResolvedIntegrityViolationReport> {
    reports
        .iter()
        .filter_map(|report| {
            let endpoint_url = endpoints.get(&report.endpoint)?.clone();
            Some(ResolvedIntegrityViolationReport {
                endpoint_name: report.endpoint.clone(),
                endpoint_url,
                report: report.clone(),
            })
        })
        .collect()
}

fn resolve_uri_reference(reference: &str, base_url: Option<&Url>) -> Option<Url> {
    match Url::parse(reference) {
        Ok(url) => Some(url),
        Err(_) if has_scheme_prefix(reference) => None,
        Err(_) => base_url?.join(reference).ok(),
    }
}

fn has_scheme_prefix(reference: &str) -> bool {
    let Some(colon) = reference.find(':') else {
        return false;
    };
    if reference[..colon].contains(['/', '?', '#']) {
        return false;
    }
    let mut chars = reference[..colon].chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
}

fn is_potentially_trustworthy(url: &Url) -> bool {
    if url.scheme() == "https" {
        return true;
    }
    if url.scheme() != "http" {
        return false;
    }

    let host = url.host().to_ascii_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host == "127.0.0.1"
        || host.starts_with("127.")
        || host == "::1"
        || host == "[::1]"
}

fn valid_dictionary_key(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_lowercase() || first == '*') {
        return false;
    }
    chars.all(|ch| {
        ch.is_ascii_lowercase()
            || ch.is_ascii_digit()
            || matches!(ch, '_' | '-' | '.' | '*')
    })
}

fn split_dictionary_members(value: &str) -> Option<Vec<&str>> {
    if value.trim().is_empty() {
        return Some(Vec::new());
    }

    let mut out = Vec::new();
    let mut start = 0usize;
    let mut quoted = false;
    let mut escaped = false;

    for (index, ch) in value.char_indices() {
        if quoted {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => quoted = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => quoted = true,
            ',' => {
                let member = value[start..index].trim();
                if member.is_empty() {
                    return None;
                }
                out.push(member);
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }

    if quoted || escaped {
        return None;
    }
    let tail = value[start..].trim();
    if tail.is_empty() {
        return None;
    }
    out.push(tail);
    Some(out)
}

fn valid_sf_string_char(ch: char) -> bool {
    ch.is_ascii() && matches!(ch as u8, 0x20..=0x7e)
}

fn parse_string_item_with_parameters(value: &str) -> Option<String> {
    if !value.starts_with('"') {
        return None;
    }

    let mut escaped = false;
    let mut closing_quote = None;
    for (index, ch) in value.char_indices().skip(1) {
        if escaped {
            if !matches!(ch, '"' | '\\') {
                return None;
            }
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => {
                closing_quote = Some(index);
                break;
            }
            _ if !valid_sf_string_char(ch) => return None,
            _ => {}
        }
    }
    if escaped {
        return None;
    }

    let end = closing_quote? + 1;
    let item = parse_quoted_string(&value[..end])?;
    if !parse_parameters(&value[end..]) {
        return None;
    }
    Some(item)
}

fn parse_parameters(mut value: &str) -> bool {
    let mut names = HashSet::new();
    loop {
        value = value.trim_start_matches([' ', '\t']);
        if value.is_empty() {
            return true;
        }
        let Some(rest) = value.strip_prefix(';') else {
            return false;
        };
        value = rest.trim_start_matches([' ', '\t']);

        let key_end = value
            .find(|ch: char| ch == '=' || ch == ';' || ch == ' ' || ch == '\t')
            .unwrap_or(value.len());
        let key = &value[..key_end];
        if !valid_dictionary_key(key) || !names.insert(key.to_string()) {
            return false;
        }
        value = &value[key_end..];
        value = value.trim_start_matches([' ', '\t']);

        if let Some(rest) = value.strip_prefix('=') {
            value = rest;
            let Some(consumed) = structured_bare_item_len(value) else {
                return false;
            };
            value = &value[consumed..];
        }
    }
}

fn structured_bare_item_len(value: &str) -> Option<usize> {
    let first = value.chars().next()?;
    match first {
        '"' => quoted_item_len(value),
        '?' => {
            if value.starts_with("?0") || value.starts_with("?1") {
                Some(2)
            } else {
                None
            }
        }
        ':' => {
            let end = value[1..].find(':')? + 2;
            let payload = &value[1..end - 1];
            if payload
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '='))
            {
                Some(end)
            } else {
                None
            }
        }
        '-' | '0'..='9' => numeric_item_len(value),
        _ if valid_token_start(first) => {
            let len = value
                .char_indices()
                .take_while(|(_, ch)| valid_token_char(*ch))
                .map(|(index, ch)| index + ch.len_utf8())
                .last()?;
            Some(len)
        }
        _ => None,
    }
}

fn quoted_item_len(value: &str) -> Option<usize> {
    let mut escaped = false;
    for (index, ch) in value.char_indices().skip(1) {
        if escaped {
            if !matches!(ch, '"' | '\\') {
                return None;
            }
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => return Some(index + 1),
            _ if !valid_sf_string_char(ch) => return None,
            _ => {}
        }
    }
    None
}

fn numeric_item_len(value: &str) -> Option<usize> {
    let bytes = value.as_bytes();
    let mut index = usize::from(bytes.first() == Some(&b'-'));
    let digits_start = index;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
    }
    let integer_digits = index - digits_start;
    if integer_digits == 0 {
        return None;
    }

    if bytes.get(index) == Some(&b'.') {
        // Structured Fields decimals have at most 12 digits before
        // the decimal point and at most three digits after it.
        if integer_digits > 12 {
            return None;
        }
        index += 1;
        let fraction_start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        let fraction_digits = index - fraction_start;
        if fraction_digits == 0 || fraction_digits > 3 {
            return None;
        }
    } else if integer_digits > 15 {
        // sf-integer is bounded to 15 decimal digits (sign excluded).
        return None;
    }
    Some(index)
}

fn valid_token_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '*'
}

fn valid_token_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric()
        || matches!(
            ch,
            '!' | '#' | '$' | '%' | '&' | '\'' | '*' | '+' | '-' | '.' | '^' | '_' | '`' | '|' | '~'
                | ':' | '/'
        )
}

fn parse_quoted_string(value: &str) -> Option<String> {
    if value.len() < 2 || !value.starts_with('"') || !value.ends_with('"') {
        return None;
    }

    let inner = &value[1..value.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut escaped = false;
    for ch in inner.chars() {
        if escaped {
            if !matches!(ch, '"' | '\\') {
                return None;
            }
            out.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => return None,
            _ if !valid_sf_string_char(ch) => return None,
            _ => out.push(ch),
        }
    }
    if escaped {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_https_dictionary_and_strips_fragments() {
        let endpoints = ReportingEndpoints::parse(
            r#"primary="https://reports.test/a#secret", backup="https://reports.test/b""#,
        );
        assert_eq!(endpoints.len(), 2);
        assert_eq!(
            endpoints.get("primary").unwrap().to_string(),
            "https://reports.test/a"
        );
    }

    #[test]
    fn accepts_structured_field_member_parameters() {
        let endpoints = ReportingEndpoints::parse(
            r#"primary="https://reports.test/a";priority=high;persist=?1;sample=0.5;label="edge", backup="https://reports.test/b";flag"#,
        );
        assert_eq!(endpoints.len(), 2);
        assert_eq!(
            endpoints.get("primary").unwrap().to_string(),
            "https://reports.test/a"
        );
    }

    #[test]
    fn structured_field_strings_reject_non_ascii_octets() {
        assert!(ReportingEndpoints::parse(
            "default=\"https://reports.test/café\""
        )
        .is_empty());
        assert!(ReportingEndpoints::parse(
            "default=\"https://reports.test/a\";label=\"é\""
        )
        .is_empty());
    }

    #[test]
    fn structured_field_numeric_parameters_enforce_wire_limits() {
        assert_eq!(
            ReportingEndpoints::parse(
                r#"a="https://reports.test/1";integer=999999999999999;decimal=999999999999.999"#
            )
            .len(),
            1
        );
        assert!(ReportingEndpoints::parse(
            r#"a="https://reports.test/1";integer=9999999999999999"#
        )
        .is_empty());
        assert!(ReportingEndpoints::parse(
            r#"a="https://reports.test/1";decimal=9999999999999.1"#
        )
        .is_empty());
        assert!(ReportingEndpoints::parse(
            r#"a="https://reports.test/1";decimal=1.2345"#
        )
        .is_empty());
    }

    #[test]
    fn malformed_structured_field_parameters_fail_closed() {
        assert!(ReportingEndpoints::parse(
            r#"a="https://reports.test/1";priority="unterminated"#
        )
        .is_empty());
        assert!(ReportingEndpoints::parse(
            r#"a="https://reports.test/1";flag;flag=?0"#
        )
        .is_empty());
        assert!(ReportingEndpoints::parse(
            r#"a="https://reports.test/1";priority=@bad"#
        )
        .is_empty());
    }

    #[test]
    fn resolves_relative_uri_references_against_response_url() {
        let base = Url::parse("https://example.test/app/page.html?old=1#fragment").unwrap();
        let endpoints = ReportingEndpoints::parse_with_base(
            r#"root="/reports", sibling="../collector?kind=csp#private", cdn="//reports.test/a""#,
            &base,
        );

        assert_eq!(endpoints.len(), 3);
        assert_eq!(
            endpoints.get("root").unwrap().to_string(),
            "https://example.test/reports"
        );
        assert_eq!(
            endpoints.get("sibling").unwrap().to_string(),
            "https://example.test/collector?kind=csp"
        );
        assert_eq!(
            endpoints.get("cdn").unwrap().to_string(),
            "https://reports.test/a"
        );
    }

    #[test]
    fn malformed_absolute_reference_is_not_reinterpreted_as_relative() {
        let base = Url::parse("https://example.test/app/page.html").unwrap();
        let endpoints = ReportingEndpoints::parse_with_base(
            r#"broken="https://reports.test:bad/report", good="/report""#,
            &base,
        );
        assert!(endpoints.get("broken").is_none());
        assert_eq!(
            endpoints.get("good").unwrap().to_string(),
            "https://example.test/report"
        );
    }

    #[test]
    fn ignores_insecure_endpoint_members() {
        let endpoints = ReportingEndpoints::parse(
            r#"secure="https://reports.test/a", insecure="http://reports.test/b""#,
        );
        assert!(endpoints.get("secure").is_some());
        assert!(endpoints.get("insecure").is_none());
    }

    #[test]
    fn allows_potentially_trustworthy_loopback_http() {
        let endpoints = ReportingEndpoints::parse(
            r#"local="http://localhost:8080/report", loopback="http://127.0.0.1/report", insecure="http://example.test/report""#,
        );
        assert!(endpoints.get("local").is_some());
        assert!(endpoints.get("loopback").is_some());
        assert!(endpoints.get("insecure").is_none());
    }

    #[test]
    fn malformed_or_duplicate_dictionary_fails_closed() {
        assert!(ReportingEndpoints::parse(
            r#"a="https://reports.test/1", a="https://reports.test/2""#
        )
        .is_empty());
        assert!(ReportingEndpoints::parse("a=https://reports.test/1").is_empty());
        assert!(ReportingEndpoints::parse(r#"a="https://reports.test/1" trailing"#).is_empty());
    }

    #[test]
    fn quoted_commas_do_not_split_members() {
        let endpoints = ReportingEndpoints::parse(
            r#"a="https://reports.test/path?x=1,2", b="https://reports.test/b""#,
        );
        assert_eq!(endpoints.len(), 2);
        assert_eq!(
            endpoints.get("a").unwrap().to_string(),
            "https://reports.test/path?x=1,2"
        );
    }
}
