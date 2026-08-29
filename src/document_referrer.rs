//! Document-owned referrer state across committed navigations.
//!
//! `RedirectReferrerState` owns one redirect chain. A browsing context also
//! needs to remember the URL and Referrer-Policy of the document that was
//! actually committed, then use that state as the source of the next
//! navigation. This module bridges those two lifetimes without reconstructing
//! browser policy from a serialized `Referer` header.

use crate::cookie_same_site::SameSiteRequestContext;
use crate::dom::Node;
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
}
