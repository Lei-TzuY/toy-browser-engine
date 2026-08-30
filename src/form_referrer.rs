//! Form-navigation referrer controls.
//!
//! HTML's `rel` link types are not limited to anchors: `<form>` also accepts
//! `rel=noreferrer`.  A form has no `referrerpolicy` attribute, so the only
//! element-level change is the `noreferrer` relationship; otherwise submission
//! inherits the committed document's current Referrer Policy.

use crate::hyperlink_referrer::rel_has_noreferrer;
use crate::net::Url;
use crate::referrer_policy::{RedirectReferrerState, ReferrerPolicy};

/// Resolve the referrer policy for one HTML form submission.
///
/// `rel=noreferrer` suppresses referrer information for the submission.
/// Other, missing, or invalid relationship tokens leave the document policy
/// unchanged. Relationship tokenization is shared with hyperlinks: ASCII
/// whitespace separates case-insensitive tokens, while commas do not.
pub fn form_referrer_policy(
    document_policy: ReferrerPolicy,
    rel: Option<&str>,
) -> ReferrerPolicy {
    if rel.is_some_and(rel_has_noreferrer) {
        ReferrerPolicy::NoReferrer
    } else {
        document_policy
    }
}

/// Build redirect-aware referrer state for one form submission.
///
/// The committed document URL remains the stable conceptual referrer source
/// across redirects, while the form's `rel` selects the initial policy for
/// this navigation only. This mirrors the existing hyperlink state boundary
/// and keeps form-only policy from mutating the committed document.
pub fn form_redirect_state(
    source: Option<&Url>,
    document_policy: ReferrerPolicy,
    rel: Option<&str>,
) -> RedirectReferrerState {
    RedirectReferrerState::new(
        source.cloned(),
        form_referrer_policy(document_policy, rel),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::FetchRequest;

    fn url(input: &str) -> Url {
        Url::parse(input).unwrap()
    }

    #[test]
    fn noreferrer_suppresses_the_document_policy() {
        assert_eq!(
            form_referrer_policy(
                ReferrerPolicy::UnsafeUrl,
                Some("external NoReFeRrEr")
            ),
            ReferrerPolicy::NoReferrer
        );
    }

    #[test]
    fn unrelated_or_missing_rel_inherits_document_policy() {
        assert_eq!(
            form_referrer_policy(ReferrerPolicy::Origin, Some("noopener external")),
            ReferrerPolicy::Origin
        );
        assert_eq!(
            form_referrer_policy(ReferrerPolicy::SameOrigin, None),
            ReferrerPolicy::SameOrigin
        );
    }

    #[test]
    fn comma_is_not_a_relationship_separator() {
        assert_eq!(
            form_referrer_policy(
                ReferrerPolicy::UnsafeUrl,
                Some("noopener,noreferrer")
            ),
            ReferrerPolicy::UnsafeUrl
        );
    }

    #[test]
    fn redirect_state_removes_a_stale_authored_referer() {
        let source = url("https://source.test/private/form?q=1#fragment");
        let mut request = FetchRequest::get(url("https://target.test/submit"));
        request
            .headers
            .insert_raw("referer", "https://attacker.invalid/forged");

        form_redirect_state(
            Some(&source),
            ReferrerPolicy::UnsafeUrl,
            Some("noreferrer"),
        )
        .prepare_request(&mut request);

        assert!(request.headers.get("referer").is_none());
    }
}
