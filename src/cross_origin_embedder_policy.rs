//! Cross-Origin-Embedder-Policy response-header parsing.
//!
//! COEP is a Structured Field Item whose bare item is one of the policy
//! tokens and whose optional `report-to` parameter names a Reporting API
//! endpoint.  The effective policy defaults to `unsafe-none` when the header
//! is absent or malformed.

use crate::cross_origin_resource_policy::CrossOriginEmbedderPolicy;
use crate::net::HeaderMap;

/// Parsed enforced or report-only COEP response policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCrossOriginEmbedderPolicy {
    pub policy: CrossOriginEmbedderPolicy,
    pub report_to: Option<String>,
}

impl Default for ParsedCrossOriginEmbedderPolicy {
    fn default() -> Self {
        Self {
            policy: CrossOriginEmbedderPolicy::UnsafeNone,
            report_to: None,
        }
    }
}

/// Parse the enforced `Cross-Origin-Embedder-Policy` response field.
pub fn parse_cross_origin_embedder_policy(
    headers: &HeaderMap,
) -> ParsedCrossOriginEmbedderPolicy {
    parse_named_coep_header(headers, "cross-origin-embedder-policy")
}

/// Parse the `Cross-Origin-Embedder-Policy-Report-Only` response field.
pub fn parse_cross_origin_embedder_policy_report_only(
    headers: &HeaderMap,
) -> ParsedCrossOriginEmbedderPolicy {
    parse_named_coep_header(headers, "cross-origin-embedder-policy-report-only")
}

fn parse_named_coep_header(headers: &HeaderMap, name: &str) -> ParsedCrossOriginEmbedderPolicy {
    let mut values = headers
        .iter()
        .filter(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
        .map(|(_, value)| value);

    let Some(value) = values.next() else {
        return ParsedCrossOriginEmbedderPolicy::default();
    };

    // A duplicated field is not a valid single Structured Field Item.  Do not
    // let HeaderMap's comma-joining turn it into a policy by accident.
    if values.next().is_some() {
        return ParsedCrossOriginEmbedderPolicy::default();
    }

    parse_coep_item(value).unwrap_or_default()
}

fn parse_coep_item(value: &str) -> Option<ParsedCrossOriginEmbedderPolicy> {
    let parts = split_parameters(value)?;
    let token = parts.first()?.trim_matches(|c| matches!(c, ' ' | '\t'));
    let policy = match token {
        "unsafe-none" => CrossOriginEmbedderPolicy::UnsafeNone,
        "require-corp" => CrossOriginEmbedderPolicy::RequireCorp,
        "credentialless" => CrossOriginEmbedderPolicy::Credentialless,
        _ => return None,
    };

    let mut report_to = None;
    for raw_parameter in parts.iter().skip(1) {
        let parameter = raw_parameter.trim_matches(|c| matches!(c, ' ' | '\t'));
        if parameter.is_empty() {
            return None;
        }
        let Some((name, raw_value)) = parameter.split_once('=') else {
            return None;
        };
        let name = name.trim_matches(|c| matches!(c, ' ' | '\t'));
        let raw_value = raw_value.trim_matches(|c| matches!(c, ' ' | '\t'));
        if !is_structured_key(name) {
            return None;
        }

        if name == "report-to" {
            if report_to.is_some() {
                return None;
            }
            report_to = Some(parse_structured_string(raw_value)?);
        } else if !is_structured_bare_item(raw_value) {
            // Unknown Structured Field parameters do not affect the policy,
            // but malformed parameter syntax invalidates the field.
            return None;
        }
    }

    Some(ParsedCrossOriginEmbedderPolicy { policy, report_to })
}

/// Split an Item from its parameters without treating a semicolon inside a
/// quoted Structured Field string as a separator.
fn split_parameters(value: &str) -> Option<Vec<&str>> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;

    for (index, byte) in value.bytes().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' if quoted => escaped = true,
            b'"' => quoted = !quoted,
            b';' if !quoted => {
                parts.push(&value[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }

    if quoted || escaped {
        return None;
    }
    parts.push(&value[start..]);
    Some(parts)
}

fn is_structured_key(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_lowercase() || first == '*')
        && chars.all(|c| {
            c.is_ascii_lowercase()
                || c.is_ascii_digit()
                || matches!(c, '_' | '-' | '.' | '*')
        })
}

fn parse_structured_string(value: &str) -> Option<String> {
    let inner = value.strip_prefix('"')?.strip_suffix('"')?;
    let mut output = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next()? {
                '\\' => output.push('\\'),
                '"' => output.push('"'),
                _ => return None,
            },
            '"' | '\r' | '\n' => return None,
            c if c.is_ascii() && !c.is_ascii_control() => output.push(c),
            _ => return None,
        }
    }
    Some(output)
}

fn is_structured_bare_item(value: &str) -> bool {
    if value == "?0" || value == "?1" {
        return true;
    }
    if value.starts_with('"') {
        return parse_structured_string(value).is_some();
    }
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        return false;
    }
    value.is_ascii()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(name: &str, values: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append_raw(name, value);
        }
        headers
    }

    #[test]
    fn missing_and_invalid_values_default_to_unsafe_none() {
        assert_eq!(
            parse_cross_origin_embedder_policy(&HeaderMap::new()),
            ParsedCrossOriginEmbedderPolicy::default()
        );
        assert_eq!(
            parse_cross_origin_embedder_policy(&headers(
                "Cross-Origin-Embedder-Policy",
                &["Require-Corp"],
            )),
            ParsedCrossOriginEmbedderPolicy::default()
        );
    }

    #[test]
    fn parses_all_policy_tokens() {
        for (value, expected) in [
            ("unsafe-none", CrossOriginEmbedderPolicy::UnsafeNone),
            ("require-corp", CrossOriginEmbedderPolicy::RequireCorp),
            ("credentialless", CrossOriginEmbedderPolicy::Credentialless),
        ] {
            let parsed = parse_cross_origin_embedder_policy(&headers(
                "Cross-Origin-Embedder-Policy",
                &[value],
            ));
            assert_eq!(parsed.policy, expected);
            assert_eq!(parsed.report_to, None);
        }
    }

    #[test]
    fn parses_report_to_and_quoted_escapes() {
        let parsed = parse_cross_origin_embedder_policy(&headers(
            "Cross-Origin-Embedder-Policy",
            &[r#"require-corp; report-to="coep\\\"endpoint""#],
        ));
        assert_eq!(parsed.policy, CrossOriginEmbedderPolicy::RequireCorp);
        assert_eq!(parsed.report_to.as_deref(), Some("coep\\\"endpoint"));
    }

    #[test]
    fn semicolon_inside_report_to_string_is_not_a_parameter_separator() {
        let parsed = parse_cross_origin_embedder_policy(&headers(
            "Cross-Origin-Embedder-Policy",
            &[r#"credentialless; report-to="a;b""#],
        ));
        assert_eq!(parsed.policy, CrossOriginEmbedderPolicy::Credentialless);
        assert_eq!(parsed.report_to.as_deref(), Some("a;b"));
    }

    #[test]
    fn duplicate_fields_and_duplicate_report_to_are_invalid() {
        assert_eq!(
            parse_cross_origin_embedder_policy(&headers(
                "Cross-Origin-Embedder-Policy",
                &["require-corp", "credentialless"],
            )),
            ParsedCrossOriginEmbedderPolicy::default()
        );
        assert_eq!(
            parse_cross_origin_embedder_policy(&headers(
                "Cross-Origin-Embedder-Policy",
                &[r#"require-corp; report-to="a"; report-to="b""#],
            )),
            ParsedCrossOriginEmbedderPolicy::default()
        );
    }

    #[test]
    fn report_only_is_parsed_independently() {
        let mut headers = HeaderMap::new();
        headers.append_raw("Cross-Origin-Embedder-Policy", "unsafe-none");
        headers.append_raw(
            "Cross-Origin-Embedder-Policy-Report-Only",
            r#"require-corp; report-to="observe""#,
        );
        assert_eq!(
            parse_cross_origin_embedder_policy(&headers).policy,
            CrossOriginEmbedderPolicy::UnsafeNone
        );
        let report_only = parse_cross_origin_embedder_policy_report_only(&headers);
        assert_eq!(report_only.policy, CrossOriginEmbedderPolicy::RequireCorp);
        assert_eq!(report_only.report_to.as_deref(), Some("observe"));
    }
}
