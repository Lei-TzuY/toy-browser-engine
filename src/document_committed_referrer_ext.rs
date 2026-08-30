// ============================================================
// document_committed_referrer_ext.rs — committed browsing policy bridge
// ============================================================

/// Freeze the referrer state a final navigation response establishes before
/// authored scripts get a chance to mutate parsed metadata.
///
/// Browser owns this value alongside its current Document. Keeping the
/// response-derived state in the browsing session avoids changing Document's
/// long-standing public field layout while still giving every later element
/// request the exact policy of the committed page.
pub(crate) fn committed_referrer_context_from_response(
    response: &crate::net::FetchResponse,
) -> crate::document_referrer::DocumentReferrerContext {
    let html = String::from_utf8_lossy(&response.body);
    let dom = crate::html::parse_html(&html);
    crate::document_referrer::DocumentReferrerContext::from_response_and_document(response, &dom)
}

impl Document {
    /// Refresh script-created images with the browsing context's committed
    /// response/meta referrer state.
    ///
    /// The underlying implementation remains the same #159 CORS/credentials
    /// path, including HSTS-effective URL classification, redirect-aware
    /// referrer recomputation and ImageCache negative caching. This bridge only
    /// supplies the long-lived document policy instead of reconstructing a
    /// default policy from the mutable live DOM.
    pub(crate) fn refresh_images_with_committed_referrer(
        &mut self,
        navigation: &crate::navigation_network::NavigationNetwork,
        referrer: &crate::document_referrer::DocumentReferrerContext,
    ) {
        self.refresh_images_with_referrer_context(navigation, referrer);
    }
}
