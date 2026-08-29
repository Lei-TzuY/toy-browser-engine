//! Document-owned referrer state across committed navigations.
//!
//! `RedirectReferrerState` owns one redirect chain. A browsing context also
//! needs to remember the URL and Referrer-Policy of the document that was
//! actually committed, then use that state as the source of the next
//! navigation. This module bridges those two lifetimes without reconstructing
//! browser policy from a serialized `Referer` header.

use crate::cookie_same_site::SameSiteRequestContext;
use crate::dom::Node;
use crate::hyperlink_referrer::hyperlink_referrer_policy;
use crate::navigation_network::NavigationNetwork;
use crate::net::{FetchError, FetchRequest, FetchResponse, Url};
use crate::referrer_meta::apply_meta_referrer_policies;
use crate::referrer_policy::{RedirectReferrerState, ReferrerPolicy};

/// Referrer state owned by one committed document.
///
/// The document URL is kept separately from the currently active policy. Each
/// outgoing navigation receives a fresh [`RedirectReferrerState`] whose stable
/// source is this committed URL. When that navigation commits, construct the
/// next context from the final response so its own `Referrer-Policy` governs
/// subsequent requests rather than leaking redirect-chain policy forward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentReferrerContext {
    source: Option<Url>,
    policy: ReferrerPolicy,
}

impl DocumentReferrerContext {
    /// Create document state from an explicit committed URL and policy.
    pub fn new(source: Option<Url>, policy: ReferrerPolicy) -> Self {
        Self { source, policy }
    }

    /// Create a normally-referring document using the modern default policy.
    pub fn from_url(source: Url) -> Self {
        Self::new(Some(source), ReferrerPolicy::default())
    }

    /// Create an environment that has no referrer source.
    pub fn no_referrer() -> Self {
        Self::new(None, ReferrerPolicy::default())
    }

    /// Build committed-document state from a final navigation response.
    ///
    /// A final response's recognized `Referrer-Policy` becomes the policy of
    /// the new document. If it has no recognized token, the engine's modern
    /// default applies. Intermediate 3xx policy changes are intentionally not
    /// inherited here: they govern only the request redirect chain.
    pub fn from_response(response: &FetchResponse) -> Self {
        Self::new(
            Some(response.url.clone()),
            ReferrerPolicy::from_response(response).unwrap_or_default(),
        )
    }

    /// Build committed-document state after the response body has been parsed.
    ///
    /// The response header establishes the document's initial policy container
    /// value. Parsed `<meta name="referrer">` elements then update that policy
    /// in document insertion order, matching HTML's metadata processing model.
    /// Invalid or empty meta values leave the current policy unchanged.
    pub fn from_response_and_document(response: &FetchResponse, document: &Node) -> Self {
        let header_policy = ReferrerPolicy::from_response(response).unwrap_or_default();
        Self::new(
            Some(response.url.clone()),
            apply_meta_referrer_policies(document, header_policy),
        )
    }

    pub fn source(&self) -> Option<&Url> {
        self.source.as_ref()
    }

    pub fn policy(&self) -> ReferrerPolicy {
        self.policy
    }

    /// Produce independent per-navigation redirect state.
    pub fn redirect_state(&self) -> RedirectReferrerState {
        RedirectReferrerState::new(self.source.clone(), self.policy)
    }

    /// Produce per-navigation redirect state for one hyperlink activation.
    ///
    /// HTML hyperlink controls are resolved before the first request is
    /// dispatched: a recognized `referrerpolicy` overrides the document's
    /// policy, while `rel=noreferrer` suppresses the referrer regardless of
    /// that attribute. The resulting policy then remains ordinary redirect
    /// state, so an intermediate HTTP `Referrer-Policy` response may update it
    /// again before the next hop.
    pub fn hyperlink_redirect_state(
        &self,
        referrerpolicy: Option<&str>,
        rel: Option<&str>,
    ) -> RedirectReferrerState {
        RedirectReferrerState::new(
            self.source.clone(),
            hyperlink_referrer_policy(self.policy, referrerpolicy, rel),
        )
    }

    /// Run one top-level navigation and return both its final response and the
    /// referrer context that should belong to that response if the caller
    /// commits it as the next document.
    ///
    /// Keeping the returned context separate from `self` lets Browser-style
    /// callers decide whether an error/status response actually commits rather
    /// than mutating the outgoing document pre-emptively.
    pub fn fetch_navigation(
        &self,
        network: &NavigationNetwork,
        request: &FetchRequest,
        context: SameSiteRequestContext,
    ) -> Result<(FetchResponse, DocumentReferrerContext), FetchError> {
        let response = network.fetch_with_referrer(
            request,
            context,
            Some(self.redirect_state()),
        )?;
        let next = Self::from_response(&response);
        Ok((response, next))
    }

    /// Run one hyperlink navigation with HTML hyperlink referrer controls.
    ///
    /// This is the transport-facing companion to
    /// [`crate::hyperlink_referrer::hyperlink_referrer_policy`]. It applies the
    /// selected hyperlink policy to the first request and carries it through
    /// redirect orchestration without mutating the committed source document.
    /// The returned context belongs to the final response only if the caller
    /// actually commits that response as the next document.
    pub fn fetch_hyperlink_navigation(
        &self,
        network: &NavigationNetwork,
        request: &FetchRequest,
        context: SameSiteRequestContext,
        referrerpolicy: Option<&str>,
        rel: Option<&str>,
    ) -> Result<(FetchResponse, DocumentReferrerContext), FetchError> {
        let response = network.fetch_with_referrer(
            request,
            context,
            Some(self.hyperlink_redirect_state(referrerpolicy, rel)),
        )?;
        let next = Self::from_response(&response);
        Ok((response, next))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::parse_html;
    use crate::net::FetchResponse;

    fn url(input: &str) -> Url {
        Url::parse(input).unwrap()
    }

    #[test]
    fn final_response_policy_becomes_the_committed_document_policy() {
        let mut response = FetchResponse::synthetic(
            url("https://example.test/final"),
            200,
            Some("text/html"),
            Vec::new(),
        );
        response
            .headers
            .append_raw("referrer-policy", "unsafe-url, no-referrer");

        let context = DocumentReferrerContext::from_response(&response);
        assert_eq!(
            context.source().map(ToString::to_string).as_deref(),
            Some("https://example.test/final")
        );
        assert_eq!(context.policy(), ReferrerPolicy::NoReferrer);
    }

    #[test]
    fn missing_or_unknown_final_policy_uses_the_modern_default() {
        let mut response = FetchResponse::synthetic(
            url("https://example.test/final"),
            200,
            Some("text/html"),
            Vec::new(),
        );
        response
            .headers
            .append_raw("referrer-policy", "future-policy");

        let context = DocumentReferrerContext::from_response(&response);
        assert_eq!(context.policy(), ReferrerPolicy::default());
    }

    #[test]
    fn parsed_meta_policy_overrides_response_header_for_committed_document() {
        let mut response = FetchResponse::synthetic(
            url("https://example.test/final"),
            200,
            Some("text/html"),
            Vec::new(),
        );
        response.headers.append_raw("referrer-policy", "origin");
        let dom = parse_html(
            r#"<html><head><meta name="referrer" content="no-referrer"></head></html>"#,
        );

        let context = DocumentReferrerContext::from_response_and_document(&response, &dom);
        assert_eq!(context.policy(), ReferrerPolicy::NoReferrer);
    }

    #[test]
    fn hyperlink_state_applies_noreferrer_without_mutating_document_policy() {
        let context = DocumentReferrerContext::new(
            Some(url("https://source.test/private")),
            ReferrerPolicy::UnsafeUrl,
        );

        let state = context.hyperlink_redirect_state(Some("unsafe-url"), Some("noreferrer"));
        assert_eq!(state.policy(), ReferrerPolicy::NoReferrer);
        assert_eq!(context.policy(), ReferrerPolicy::UnsafeUrl);
    }
}
