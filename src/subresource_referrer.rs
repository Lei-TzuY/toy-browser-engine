//! Referrer-policy selection for element-initiated subresource requests.
//!
//! HTML exposes `referrerpolicy` on several fetching elements such as images,
//! scripts, stylesheets, and iframes. The attribute uses the same modern
//! Referrer Policy keyword grammar as hyperlink `referrerpolicy`, but unlike a
//! hyperlink there is no `rel=noreferrer` override involved in ordinary
//! subresource fetching. This module keeps that policy choice separate from
//! transport code and prepares the actual `Referer` header from browser-owned
//! document state.

use crate::hyperlink_referrer::parse_referrer_policy_attribute;
use crate::net::{FetchRequest, Url};
use crate::referrer_policy::{RedirectReferrerState, ReferrerPolicy};

/// Resolve an element's optional `referrerpolicy` against the owning document.
///
/// Missing or invalid attribute values inherit the document policy. A
/// recognized value overrides it for this fetch only.
pub fn subresource_referrer_policy(
    document_policy: ReferrerPolicy,
    referrerpolicy: Option<&str>,
) -> ReferrerPolicy {
    referrerpolicy
        .and_then(parse_referrer_policy_attribute)
        .unwrap_or(document_policy)
}

/// Build redirect-aware referrer state for one subresource fetch.
///
/// The original document URL is retained independently from the serialized
/// `Referer` header so redirect responses may subsequently tighten or otherwise
/// update the policy without losing the conceptual referrer source.
pub fn subresource_redirect_state(
    document_url: Option<&Url>,
    document_policy: ReferrerPolicy,
    referrerpolicy: Option<&str>,
) -> RedirectReferrerState {
    RedirectReferrerState::new(
        document_url.cloned(),
        subresource_referrer_policy(document_policy, referrerpolicy),
    )
}

/// Replace any authored/stale `Referer` on an element-initiated request with
/// the value allowed by the document URL and effective subresource policy.
///
/// This helper prepares the first hop. Callers that own redirect orchestration
/// should retain [`subresource_redirect_state`] and use its redirect-response
/// update step before preparing later hops.
pub fn prepare_subresource_request(
    request: &mut FetchRequest,
    document_url: Option<&Url>,
    document_policy: ReferrerPolicy,
    referrerpolicy: Option<&str>,
) {
    subresource_redirect_state(document_url, document_policy, referrerpolicy)
        .prepare_request(request);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(input: &str) -> Url {
        Url::parse(input).unwrap()
    }

    #[test]
    fn recognized_attribute_overrides_document_policy() {
        assert_eq!(
            subresource_referrer_policy(ReferrerPolicy::NoReferrer, Some("unsafe-url")),
            ReferrerPolicy::UnsafeUrl
        );
    }

    #[test]
    fn invalid_attribute_inherits_document_policy() {
        assert_eq!(
            subresource_referrer_policy(ReferrerPolicy::Origin, Some(" future-policy ")),
            ReferrerPolicy::Origin
        );
    }

    #[test]
    fn request_preparation_removes_stale_referer_and_applies_override() {
        let source = url("https://source.test/private/page?q=1#secret");
        let mut request = FetchRequest::get(url("https://cdn.test/app.js"));
        request
            .headers
            .insert_raw("referer", "https://attacker.invalid/forged");

        prepare_subresource_request(
            &mut request,
            Some(&source),
            ReferrerPolicy::NoReferrer,
            Some("unsafe-url"),
        );

        assert_eq!(
            request.headers.get("referer").as_deref(),
            Some("https://source.test/private/page?q=1")
        );
    }

    #[test]
    fn no_document_source_always_omits_referer() {
        let mut request = FetchRequest::get(url("https://cdn.test/app.js"));
        request
            .headers
            .insert_raw("referer", "https://attacker.invalid/forged");

        prepare_subresource_request(
            &mut request,
            None,
            ReferrerPolicy::UnsafeUrl,
            Some("unsafe-url"),
        );

        assert_eq!(request.headers.get("referer"), None);
    }
}
