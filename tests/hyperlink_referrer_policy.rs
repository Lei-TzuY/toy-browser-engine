use browser_engine::{
    hyperlink_referrer_policy, parse_referrer_policy_attribute, rel_has_noreferrer, ReferrerPolicy,
};

#[test]
fn recognized_attribute_overrides_document_policy() {
    assert_eq!(
        hyperlink_referrer_policy(
            ReferrerPolicy::StrictOriginWhenCrossOrigin,
            Some("unsafe-url"),
            None,
        ),
        ReferrerPolicy::UnsafeUrl
    );
}

#[test]
fn invalid_attribute_inherits_document_policy() {
    assert_eq!(
        hyperlink_referrer_policy(
            ReferrerPolicy::Origin,
            Some(" origin "),
            Some("external"),
        ),
        ReferrerPolicy::Origin
    );
    assert_eq!(
        parse_referrer_policy_attribute("origin, no-referrer"),
        None
    );
}

#[test]
fn noreferrer_relationship_suppresses_even_unsafe_url() {
    assert_eq!(
        hyperlink_referrer_policy(
            ReferrerPolicy::UnsafeUrl,
            Some("unsafe-url"),
            Some("noopener\nNOREFERRER external"),
        ),
        ReferrerPolicy::NoReferrer
    );
}

#[test]
fn relationship_tokenization_uses_ascii_whitespace_not_commas() {
    assert!(rel_has_noreferrer("external\tnoreferrer"));
    assert!(!rel_has_noreferrer("external,noreferrer"));
}

#[test]
fn legacy_meta_spellings_are_not_valid_attribute_keywords() {
    for legacy in ["never", "default", "always", "origin-when-crossorigin"] {
        assert_eq!(parse_referrer_policy_attribute(legacy), None, "{legacy}");
    }
}
