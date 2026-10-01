// ============================================================
//  cookie_request_policy_ext.rs — request-context cookie filtering
// ============================================================

impl CookieJar {
    /// Formats the `Cookie:` header for an outgoing HTTP request while applying
    /// RFC6265bis SameSite request-context policy to every otherwise matching
    /// stored cookie.
    ///
    /// This is intentionally separate from `get_http_cookie_header`: existing
    /// callers keep their historical same-site behavior, while request-aware
    /// network policy can opt into explicit site/navigation/method filtering.
    pub fn get_http_cookie_header_for_context(
        &self,
        url: &Url,
        now_ms: u64,
        context: crate::cookie_same_site::SameSiteRequestContext,
    ) -> Option<String> {
        if !matches!(url.scheme(), "http" | "https") {
            return None;
        }

        let now_ms = self.effective_now_ms(now_ms);
        let host = url.host();
        let path = url.path();
        let is_secure = url.scheme() == "https";

        let mut matching: Vec<&Cookie> = self
            .cookies
            .iter()
            .filter(|cookie| !cookie.is_expired(now_ms))
            .filter(|cookie| !cookie.secure || is_secure)
            .filter(|cookie| cookie.matches_domain(host))
            .filter(|cookie| cookie.matches_path(path))
            .filter(|cookie| crate::cookie_same_site::cookie_allows_request(cookie, context))
            .collect();

        matching.sort_by_key(|cookie| std::cmp::Reverse(cookie.path.len()));
        let value = matching
            .into_iter()
            .map(serialize_cookie_pair)
            .collect::<Vec<_>>()
            .join("; ");

        (!value.is_empty()).then_some(value)
    }
}
