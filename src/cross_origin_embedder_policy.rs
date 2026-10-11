//! Cross-Origin-Embedder-Policy response-header parsing.
//!
//! COEP is a Structured Field Item whose bare item is one of the policy
//! tokens and whose optional `report-to` parameter names a Reporting API
//! endpoint.  The effective policy defaults to `unsafe-none` when the header
//! is absent or malformed.

use crate::cross_origin_resource_policy::CrossOriginEmbedderPolicy;
use crate::http_structured_field::{
    parse_quoted_string as parse_structured_string, structured_bare_item_len,
};
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
pub fn parse_cross_origin_embedder_policy(headers: &HeaderMap) -> ParsedCrossOriginEmbedderPolicy {
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
        let (name, raw_value) = match parameter.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            // A parameter without "=" is the Structured Field Boolean true.
            None => (parameter, None),
        };
        let name = name.trim_matches(|c| matches!(c, ' ' | '\t'));
        let raw_value = raw_value.map(|value| value.trim_matches(|c| matches!(c, ' ' | '\t')));
        if !is_structured_key(name) {
            return None;
        }
        if let Some(value) = raw_value {
            if structured_bare_item_len(value) != Some(value.len()) {
                return None;
            }
        }
        if name == "report-to" {
            // Parameter maps retain the last value. Only an ASCII String names
            // a reporting endpoint; other valid types leave the policy intact.
            report_to = match raw_value {
                Some(value) if value.starts_with('"') => Some(parse_structured_string(value)?),
                _ => None,
            };
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
    let mut display_string = false;
    let mut escaped = false;

    for (index, byte) in value.bytes().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' if quoted && !display_string => escaped = true,
            b'"' => {
                if quoted {
                    quoted = false;
                } else {
                    quoted = true;
                    display_string = value[..index].ends_with('%');
                }
            }
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
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.' | '*')
        })
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
        for value in [
            "require-corp; future=?2",
            "require-corp; future=?1suffix",
            "require-corp; report-to=?2",
            "require-corp; future=@1.5",
            r#"require-corp; future=%"%ff""#,
        ] {
            assert_eq!(
                parse_cross_origin_embedder_policy(&headers(
                    "cross-origin-embedder-policy",
                    &[value]
                )),
                ParsedCrossOriginEmbedderPolicy::default(),
                "{value}",
            );
        }
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
    fn duplicate_fields_are_invalid_and_last_report_to_parameter_wins() {
        assert_eq!(
            parse_cross_origin_embedder_policy(&headers(
                "Cross-Origin-Embedder-Policy",
                &["require-corp", "credentialless"],
            )),
            ParsedCrossOriginEmbedderPolicy::default()
        );
        let parsed = parse_cross_origin_embedder_policy(&headers(
            "Cross-Origin-Embedder-Policy",
            &[r#"require-corp; report-to="a"; report-to="b""#],
        ));
        assert_eq!(parsed.policy, CrossOriginEmbedderPolicy::RequireCorp);
        assert_eq!(parsed.report_to.as_deref(), Some("b"));
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
