//! HTML `<meta name="referrer">` processing for committed documents.
//!
//! The HTTP `Referrer-Policy` header establishes a document's initial policy,
//! but HTML may update the policy container as referrer meta elements are
//! inserted. During initial parsing, processing elements in document order
//! models that insertion order: each recognized meta policy replaces the
//! current policy and invalid values leave it unchanged.

use crate::dom::Node;
use crate::referrer_policy::ReferrerPolicy;

/// Apply every parsed `<meta name="referrer">` update to `initial`.
///
/// HTML's referrer metadata processing is deliberately stricter than the HTTP
/// header grammar: the `content` value is ASCII-lowercased but not trimmed or
/// split on commas. Legacy HTML spellings are supported for compatibility.
pub fn apply_meta_referrer_policies(root: &Node, initial: ReferrerPolicy) -> ReferrerPolicy {
    let mut policy = initial;
    visit(root, &mut policy);
    policy
}

fn visit(node: &Node, policy: &mut ReferrerPolicy) {
    if let Some(element) = node.as_element() {
        if element.tag_name.eq_ignore_ascii_case("meta")
            && element
                .get_attr("name")
                .is_some_and(|name| name.eq_ignore_ascii_case("referrer"))
        {
            if let Some(content) = element.get_attr("content") {
                if let Some(next) = parse_meta_policy(content) {
                    *policy = next;
                }
            }
        }
    }

    for child in &node.children {
        visit(child, policy);
    }
}

fn parse_meta_policy(content: &str) -> Option<ReferrerPolicy> {
    if content.is_empty() {
        return None;
    }

    match content.to_ascii_lowercase().as_str() {
        // Legacy HTML values retained by the HTML Standard.
        "never" => Some(ReferrerPolicy::NoReferrer),
        "default" => Some(ReferrerPolicy::default()),
        "always" => Some(ReferrerPolicy::UnsafeUrl),
        "origin-when-crossorigin" => Some(ReferrerPolicy::OriginWhenCrossOrigin),

        // Modern policy values. Keep these exact rather than using
        // ReferrerPolicy::parse_token(), which intentionally trims HTTP tokens.
        "no-referrer" => Some(ReferrerPolicy::NoReferrer),
        "no-referrer-when-downgrade" => Some(ReferrerPolicy::NoReferrerWhenDowngrade),
        "origin" => Some(ReferrerPolicy::Origin),
        "origin-when-cross-origin" => Some(ReferrerPolicy::OriginWhenCrossOrigin),
        "same-origin" => Some(ReferrerPolicy::SameOrigin),
        "strict-origin" => Some(ReferrerPolicy::StrictOrigin),
        "strict-origin-when-cross-origin" => Some(ReferrerPolicy::StrictOriginWhenCrossOrigin),
        "unsafe-url" => Some(ReferrerPolicy::UnsafeUrl),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::parse_html;

    #[test]
    fn later_valid_meta_updates_replace_earlier_ones() {
        let dom = parse_html(
            r#"<html><head>
                <meta name="referrer" content="never">
                <meta name="referrer" content="future-policy">
                <meta name="referrer" content="always">
            </head></html>"#,
        );

        assert_eq!(
            apply_meta_referrer_policies(&dom, ReferrerPolicy::Origin),
            ReferrerPolicy::UnsafeUrl
        );
    }

    #[test]
    fn legacy_default_restores_the_modern_default() {
        let dom = parse_html(
            r#"<meta name="referrer" content="default">"#,
        );
        assert_eq!(
            apply_meta_referrer_policies(&dom, ReferrerPolicy::NoReferrer),
            ReferrerPolicy::default()
        );
    }

    #[test]
    fn meta_matching_is_case_insensitive() {
        let dom = parse_html(
            r#"<META NAME="ReFeRrEr" CONTENT="STRICT-ORIGIN">"#,
        );
        assert_eq!(
            apply_meta_referrer_policies(&dom, ReferrerPolicy::UnsafeUrl),
            ReferrerPolicy::StrictOrigin
        );
    }

    #[test]
    fn whitespace_and_comma_lists_are_not_http_header_syntax() {
        let dom = parse_html(
            r#"<meta name="referrer" content=" no-referrer ">
               <meta name="referrer" content="origin,unsafe-url">"#,
        );
        assert_eq!(
            apply_meta_referrer_policies(&dom, ReferrerPolicy::OriginWhenCrossOrigin),
            ReferrerPolicy::OriginWhenCrossOrigin
        );
    }

    #[test]
    fn empty_or_missing_content_does_not_change_policy() {
        let dom = parse_html(
            r#"<meta name="referrer"><meta name="referrer" content="">"#,
        );
        assert_eq!(
            apply_meta_referrer_policies(&dom, ReferrerPolicy::SameOrigin),
            ReferrerPolicy::SameOrigin
        );
    }
}
