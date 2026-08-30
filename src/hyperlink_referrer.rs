//! Hyperlink-specific referrer policy resolution.
//!
//! HTML exposes two independent controls on hyperlink navigations:
//! `referrerpolicy`, an enumerated attribute whose recognized values select a
//! Referrer Policy, and `rel=noreferrer`, a link type that suppresses the
//! referrer entirely.  Keep this grammar separate from the HTTP header and
//! `<meta name="referrer">` grammars: HTML enumerated attributes are matched as
//! exact ASCII-case-insensitive keywords rather than comma-separated lists or
//! whitespace-trimmed header tokens.

use crate::referrer_policy::ReferrerPolicy;

/// Parse one HTML `referrerpolicy` attribute value.
///
/// The attribute is an enumerated attribute.  Known keywords are matched
/// ASCII-case-insensitively, but surrounding whitespace, comma-separated
/// lists, and the legacy spellings accepted by `<meta name="referrer">` are
/// invalid here.  Missing/empty/invalid values therefore return `None`, so the
/// caller can inherit its document policy.
pub fn parse_referrer_policy_attribute(value: &str) -> Option<ReferrerPolicy> {
    if value.eq_ignore_ascii_case("no-referrer") {
        Some(ReferrerPolicy::NoReferrer)
    } else if value.eq_ignore_ascii_case("no-referrer-when-downgrade") {
        Some(ReferrerPolicy::NoReferrerWhenDowngrade)
    } else if value.eq_ignore_ascii_case("origin") {
        Some(ReferrerPolicy::Origin)
    } else if value.eq_ignore_ascii_case("origin-when-cross-origin") {
        Some(ReferrerPolicy::OriginWhenCrossOrigin)
    } else if value.eq_ignore_ascii_case("same-origin") {
        Some(ReferrerPolicy::SameOrigin)
    } else if value.eq_ignore_ascii_case("strict-origin") {
        Some(ReferrerPolicy::StrictOrigin)
    } else if value.eq_ignore_ascii_case("strict-origin-when-cross-origin") {
        Some(ReferrerPolicy::StrictOriginWhenCrossOrigin)
    } else if value.eq_ignore_ascii_case("unsafe-url") {
        Some(ReferrerPolicy::UnsafeUrl)
    } else {
        None
    }
}

/// Whether an HTML `rel` value contains the `noreferrer` link type.
///
/// Link types are an unordered set of unique ASCII-case-insensitive tokens
/// separated by ASCII whitespace.  Commas are ordinary token characters and
/// therefore do not split the relationship list.
pub fn rel_has_noreferrer(rel: &str) -> bool {
    rel.split(|ch| matches!(ch, '\t' | '\n' | '\x0C' | '\r' | ' '))
        .filter(|token| !token.is_empty())
        .any(|token| token.eq_ignore_ascii_case("noreferrer"))
}

/// Resolve the policy for following one hyperlink.
///
/// `rel=noreferrer` is the strongest signal and always suppresses the
/// referrer.  Otherwise a recognized `referrerpolicy` value overrides the
/// document's current policy; missing or invalid attribute values inherit it.
pub fn hyperlink_referrer_policy(
    document_policy: ReferrerPolicy,
    referrerpolicy: Option<&str>,
    rel: Option<&str>,
) -> ReferrerPolicy {
    if rel.is_some_and(rel_has_noreferrer) {
        return ReferrerPolicy::NoReferrer;
    }

    referrerpolicy
        .and_then(parse_referrer_policy_attribute)
        .unwrap_or(document_policy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attribute_matches_known_keywords_ascii_case_insensitively() {
        assert_eq!(
            parse_referrer_policy_attribute("StRiCt-OrIgIn"),
            Some(ReferrerPolicy::StrictOrigin)
        );
    }

    #[test]
    fn attribute_does_not_reuse_header_or_meta_tokenization_rules() {
        assert_eq!(parse_referrer_policy_attribute(" origin "), None);
        assert_eq!(parse_referrer_policy_attribute("origin, no-referrer"), None);
        assert_eq!(parse_referrer_policy_attribute("always"), None);
    }

    #[test]
    fn rel_uses_ascii_whitespace_tokens() {
        assert!(rel_has_noreferrer("noopener\tNoReFeRrEr external"));
        assert!(!rel_has_noreferrer("noopener,noreferrer"));
        assert!(!rel_has_noreferrer("noreferrerish"));
    }

    #[test]
    fn noreferrer_wins_over_an_explicit_attribute() {
        assert_eq!(
            hyperlink_referrer_policy(
                ReferrerPolicy::Origin,
                Some("unsafe-url"),
                Some("external noreferrer"),
            ),
            ReferrerPolicy::NoReferrer
        );
    }

    #[test]
    fn invalid_or_missing_attribute_inherits_document_policy() {
        assert_eq!(
            hyperlink_referrer_policy(ReferrerPolicy::SameOrigin, Some("future-policy"), None),
            ReferrerPolicy::SameOrigin
        );
        assert_eq!(
            hyperlink_referrer_policy(ReferrerPolicy::Origin, None, Some("noopener")),
            ReferrerPolicy::Origin
        );
    }
}
