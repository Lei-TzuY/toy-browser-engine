// ============================================================
//  cookie_same_site.rs — RFC 6265bis SameSite request policy
// ============================================================

use crate::cookie::{Cookie, SameSite};
use crate::net::fetch::Method;

/// The request context needed to decide whether a stored cookie's SameSite
/// attribute permits it to accompany an HTTP request.
///
/// Site computation is intentionally kept outside this type. The browser/session
/// layer owns the initiator and target URLs and can therefore decide whether the
/// request is same-site without teaching the cookie store about navigation,
/// frames, or future public-suffix-list machinery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SameSiteRequestContext {
    /// Whether the request's initiator and target are same-site.
    pub same_site: bool,
    /// Whether this request is the user-visible top-level navigation itself.
    pub top_level_navigation: bool,
    /// HTTP method of the outgoing request.
    pub method: Method,
}

impl SameSiteRequestContext {
    pub const fn new(
        same_site: bool,
        top_level_navigation: bool,
        method: Method,
    ) -> SameSiteRequestContext {
        SameSiteRequestContext {
            same_site,
            top_level_navigation,
            method,
        }
    }

    /// Same-site subresource/fetch context.
    pub const fn same_site(method: Method) -> SameSiteRequestContext {
        SameSiteRequestContext::new(true, false, method)
    }

    /// Cross-site subresource/fetch context.
    pub const fn cross_site_subresource(method: Method) -> SameSiteRequestContext {
        SameSiteRequestContext::new(false, false, method)
    }

    /// Cross-site top-level navigation context.
    pub const fn cross_site_navigation(method: Method) -> SameSiteRequestContext {
        SameSiteRequestContext::new(false, true, method)
    }

    /// Fetch's safe-method subset supported by this engine.
    ///
    /// RFC 9110 also defines OPTIONS and TRACE as safe, but this engine's public
    /// Method enum does not expose either method. GET and HEAD are therefore the
    /// complete safe-method set representable here.
    pub const fn is_safe_method(self) -> bool {
        matches!(self.method, Method::Get | Method::Head)
    }
}

/// Return whether a SameSite policy permits a cookie in this request context.
///
/// This models RFC 6265bis enforcement for explicit Strict/Lax/None values:
///
/// - Strict: same-site requests only.
/// - Lax: same-site requests, plus cross-site top-level safe navigations.
/// - None: no SameSite request restriction (the parser separately requires
///   Secure before such a cookie can enter the jar).
///
/// The cookie parser currently maps an omitted/unknown SameSite attribute to
/// `SameSite::Lax`; consequently the optional "Lax-allowing-unsafe" grace mode
/// for recently-created default cookies is deliberately not invented here.
pub const fn same_site_allows(policy: SameSite, context: SameSiteRequestContext) -> bool {
    match policy {
        SameSite::Strict => context.same_site,
        SameSite::Lax => {
            context.same_site || (context.top_level_navigation && context.is_safe_method())
        }
        SameSite::None => true,
    }
}

/// Convenience wrapper for a stored cookie.
pub const fn cookie_allows_request(
    cookie: &Cookie,
    context: SameSiteRequestContext,
) -> bool {
    same_site_allows(cookie.same_site, context)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(same_site: bool, top_level: bool, method: Method) -> SameSiteRequestContext {
        SameSiteRequestContext::new(same_site, top_level, method)
    }

    #[test]
    fn strict_requires_same_site_for_every_request_shape() {
        assert!(same_site_allows(
            SameSite::Strict,
            context(true, false, Method::Post)
        ));
        assert!(!same_site_allows(
            SameSite::Strict,
            context(false, true, Method::Get)
        ));
    }

    #[test]
    fn lax_allows_cross_site_top_level_safe_navigation_only() {
        assert!(same_site_allows(
            SameSite::Lax,
            context(false, true, Method::Get)
        ));
        assert!(same_site_allows(
            SameSite::Lax,
            context(false, true, Method::Head)
        ));
        assert!(!same_site_allows(
            SameSite::Lax,
            context(false, false, Method::Get)
        ));
        assert!(!same_site_allows(
            SameSite::Lax,
            context(false, true, Method::Post)
        ));
    }

    #[test]
    fn none_does_not_apply_a_same_site_request_filter() {
        assert!(same_site_allows(
            SameSite::None,
            context(false, false, Method::Post)
        ));
    }
}