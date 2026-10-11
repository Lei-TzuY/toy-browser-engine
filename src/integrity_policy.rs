//! Integrity-Policy parsing and request gating for Subresource Integrity.
//!
//! The current SRI specification defines `Integrity-Policy` and
//! `Integrity-Policy-Report-Only` as Structured Field dictionaries whose values
//! are inner lists of tokens. This module implements the policy data model and
//! the enforcement decision independently from concrete document/reporting
//! plumbing so loaders can share one standards-oriented implementation.

use std::collections::HashSet;

/// A source recognized by the current Integrity-Policy specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrityPolicySource {
    Inline,
}

/// A request destination controlled by Integrity-Policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrityPolicyDestination {
    Script,
    Style,
    Other,
}

/// Request mode information needed by the SRI policy gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrityPolicyRequestMode {
    Cors,
    SameOrigin,
    NoCors,
    Other,
}

/// Parsed Integrity-Policy state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IntegrityPolicy {
    pub sources: Vec<IntegrityPolicySource>,
    pub blocked_destinations: Vec<IntegrityPolicyDestination>,
    pub endpoints: Vec<String>,
}

impl IntegrityPolicy {
    /// Parse one `Integrity-Policy` or `Integrity-Policy-Report-Only` field value.
    ///
    /// Per the SRI processing algorithm, an absent `sources` member defaults to
    /// `inline`. Unknown source/destination tokens are ignored. Malformed
    /// Structured Field input is treated as an empty dictionary, which is
    /// therefore also the harmless `sources=(inline)` / no-blocked-destination
    /// default.
    pub fn parse(value: &str) -> Self {
        let dictionary = parse_dictionary(value).unwrap_or_default();
        let mut policy = IntegrityPolicy::default();

        match dictionary.iter().find(|(name, _)| name == "sources") {
            None => policy.sources.push(IntegrityPolicySource::Inline),
            Some((_, values)) if values.iter().any(|value| value == "inline") => {
                policy.sources.push(IntegrityPolicySource::Inline)
            }
            Some(_) => {}
        }

        if let Some((_, values)) = dictionary
            .iter()
            .find(|(name, _)| name == "blocked-destinations")
        {
            if values.iter().any(|value| value == "script") {
                policy
                    .blocked_destinations
                    .push(IntegrityPolicyDestination::Script);
            }
            if values.iter().any(|value| value == "style") {
                policy
                    .blocked_destinations
                    .push(IntegrityPolicyDestination::Style);
            }
        }

        if let Some((_, values)) = dictionary.iter().find(|(name, _)| name == "endpoints") {
            policy.endpoints = values.clone();
        }

        policy
    }

    pub fn blocks_destination(&self, destination: IntegrityPolicyDestination) -> bool {
        self.sources.contains(&IntegrityPolicySource::Inline)
            && self.blocked_destinations.contains(&destination)
    }
}

/// Result of applying enforced and report-only Integrity-Policy state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IntegrityPolicyDecision {
    pub blocked: bool,
    pub enforced_violation: bool,
    pub report_only_violation: bool,
}

/// Apply the current SRI "Should request be blocked by Integrity Policy" gate.
///
/// Valid integrity metadata on a CORS/same-origin request wins before policy
/// lookup, and local URLs are exempt. Otherwise enforced policy may block while
/// report-only policy is observable without changing the allow/block result.
pub fn evaluate_integrity_policy(
    enforced: &IntegrityPolicy,
    report_only: &IntegrityPolicy,
    destination: IntegrityPolicyDestination,
    has_valid_integrity_metadata: bool,
    mode: IntegrityPolicyRequestMode,
    is_local: bool,
) -> IntegrityPolicyDecision {
    if (has_valid_integrity_metadata
        && matches!(
            mode,
            IntegrityPolicyRequestMode::Cors | IntegrityPolicyRequestMode::SameOrigin
        ))
        || is_local
    {
        return IntegrityPolicyDecision::default();
    }

    let enforced_violation = enforced.blocks_destination(destination);
    let report_only_violation = report_only.blocks_destination(destination);
    IntegrityPolicyDecision {
        blocked: enforced_violation,
        enforced_violation,
        report_only_violation,
    }
}

/// Parse the narrow Structured Fields shape required by SRI:
/// `key=(token token), key2=(token)`.
///
/// Dictionary member names and values are kept case-sensitive, duplicate keys
/// invalidate the field, and quoted strings/bare items are rejected because SRI
/// requires every member value to be an inner list of tokens.
fn parse_dictionary(value: &str) -> Result<Vec<(String, Vec<String>)>, ()> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    let members = split_members(trimmed)?;
    let mut seen = HashSet::new();
    let mut result = Vec::with_capacity(members.len());

    for member in members {
        let (name, raw_value) = member.split_once('=').ok_or(())?;
        let name = name.trim();
        if !is_token(name) || !seen.insert(name.to_string()) {
            return Err(());
        }

        let raw_value = raw_value.trim();
        if !raw_value.starts_with('(') || !raw_value.ends_with(')') {
            return Err(());
        }
        let inside = &raw_value[1..raw_value.len() - 1];
        if inside.contains('(') || inside.contains(')') {
            return Err(());
        }

        let mut values = Vec::new();
        for item in inside.split_ascii_whitespace() {
            if !is_token(item) {
                return Err(());
            }
            values.push(item.to_string());
        }
        result.push((name.to_string(), values));
    }

    Ok(result)
}

fn split_members(value: &str) -> Result<Vec<&str>, ()> {
    let mut result = Vec::new();
    let mut depth = 0u8;
    let mut start = 0usize;

    for (index, ch) in value.char_indices() {
        match ch {
            '(' => {
                if depth != 0 {
                    return Err(());
                }
                depth = 1;
            }
            ')' => {
                if depth != 1 {
                    return Err(());
                }
                depth = 0;
            }
            ',' if depth == 0 => {
                let member = value[start..index].trim();
                if member.is_empty() {
                    return Err(());
                }
                result.push(member);
                start = index + 1;
            }
            _ => {}
        }
    }

    if depth != 0 {
        return Err(());
    }
    let member = value[start..].trim();
    if member.is_empty() {
        return Err(());
    }
    result.push(member);
    Ok(result)
}

fn is_token(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '*') {
        return false;
    }
    chars.all(|ch| {
        ch.is_ascii_alphanumeric()
            || matches!(ch, '_' | '-' | '.' | '*' | '/' | ':' )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_current_policy_members() {
        let policy = IntegrityPolicy::parse(
            "blocked-destinations=(script style), endpoints=(primary backup)",
        );
        assert_eq!(policy.sources, vec![IntegrityPolicySource::Inline]);
        assert_eq!(
            policy.blocked_destinations,
            vec![
                IntegrityPolicyDestination::Script,
                IntegrityPolicyDestination::Style
            ]
        );
        assert_eq!(policy.endpoints, vec!["primary", "backup"]);
    }

    #[test]
    fn explicit_empty_sources_disables_inline_source() {
        let policy = IntegrityPolicy::parse("sources=(), blocked-destinations=(script)");
        assert!(policy.sources.is_empty());
        assert!(!policy.blocks_destination(IntegrityPolicyDestination::Script));
    }

    #[test]
    fn malformed_or_duplicate_dictionary_is_harmless_default() {
        for value in [
            "blocked-destinations=script",
            "blocked-destinations=(script",
            "blocked-destinations=(script), blocked-destinations=(style)",
            "endpoints=(\"quoted\")",
        ] {
            let policy = IntegrityPolicy::parse(value);
            assert_eq!(policy.sources, vec![IntegrityPolicySource::Inline]);
            assert!(policy.blocked_destinations.is_empty());
            assert!(policy.endpoints.is_empty());
        }
    }

    #[test]
    fn enforced_and_report_only_decisions_are_separate() {
        let enforced = IntegrityPolicy::parse("blocked-destinations=(script)");
        let report = IntegrityPolicy::parse("blocked-destinations=(style)");

        assert_eq!(
            evaluate_integrity_policy(
                &enforced,
                &report,
                IntegrityPolicyDestination::Script,
                false,
                IntegrityPolicyRequestMode::NoCors,
                false,
            ),
            IntegrityPolicyDecision {
                blocked: true,
                enforced_violation: true,
                report_only_violation: false,
            }
        );
        assert_eq!(
            evaluate_integrity_policy(
                &enforced,
                &report,
                IntegrityPolicyDestination::Style,
                false,
                IntegrityPolicyRequestMode::NoCors,
                false,
            ),
            IntegrityPolicyDecision {
                blocked: false,
                enforced_violation: false,
                report_only_violation: true,
            }
        );
    }

    #[test]
    fn valid_cors_integrity_and_local_urls_short_circuit_policy() {
        let enforced = IntegrityPolicy::parse("blocked-destinations=(script style)");
        let empty = IntegrityPolicy::default();
        assert_eq!(
            evaluate_integrity_policy(
                &enforced,
                &empty,
                IntegrityPolicyDestination::Script,
                true,
                IntegrityPolicyRequestMode::Cors,
                false,
            ),
            IntegrityPolicyDecision::default()
        );
        assert_eq!(
            evaluate_integrity_policy(
                &enforced,
                &empty,
                IntegrityPolicyDestination::Style,
                false,
                IntegrityPolicyRequestMode::NoCors,
                true,
            ),
            IntegrityPolicyDecision::default()
        );
    }
}
