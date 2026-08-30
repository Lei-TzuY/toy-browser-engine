use browser_engine::cookie::{Cookie, SameSite};
use browser_engine::cookie_same_site::{
    cookie_allows_request, same_site_allows, SameSiteRequestContext,
};
use browser_engine::net::fetch::Method;

fn cookie(policy: SameSite) -> Cookie {
    Cookie {
        name: "sid".into(),
        value: "abc".into(),
        domain: "example.test".into(),
        host_only: true,
        path: "/".into(),
        expires_at_ms: None,
        secure: policy == SameSite::None,
        http_only: false,
        same_site: policy,
    }
}

#[test]
fn same_site_requests_allow_all_three_policies() {
    let ctx = SameSiteRequestContext::same_site(Method::Post);
    for policy in [SameSite::Strict, SameSite::Lax, SameSite::None] {
        assert!(
            same_site_allows(policy, ctx),
            "same-site request unexpectedly blocked for {policy:?}"
        );
    }
}

#[test]
fn strict_cookie_is_not_sent_on_cross_site_navigation() {
    let ctx = SameSiteRequestContext::cross_site_navigation(Method::Get);
    assert!(!cookie_allows_request(&cookie(SameSite::Strict), ctx));
}

#[test]
fn lax_cookie_allows_cross_site_top_level_get_and_head() {
    for method in [Method::Get, Method::Head] {
        let ctx = SameSiteRequestContext::cross_site_navigation(method);
        assert!(cookie_allows_request(&cookie(SameSite::Lax), ctx));
    }
}

#[test]
fn lax_cookie_blocks_cross_site_subresource_even_when_method_is_safe() {
    let ctx = SameSiteRequestContext::cross_site_subresource(Method::Get);
    assert!(!cookie_allows_request(&cookie(SameSite::Lax), ctx));
}

#[test]
fn lax_cookie_blocks_cross_site_top_level_unsafe_methods() {
    for method in [Method::Post, Method::Put, Method::Delete, Method::Patch] {
        let ctx = SameSiteRequestContext::cross_site_navigation(method);
        assert!(
            !cookie_allows_request(&cookie(SameSite::Lax), ctx),
            "unsafe method {method:?} unexpectedly passed Lax"
        );
    }
}

#[test]
fn none_cookie_is_not_restricted_by_same_site_request_context() {
    let ctx = SameSiteRequestContext::cross_site_subresource(Method::Post);
    assert!(cookie_allows_request(&cookie(SameSite::None), ctx));
}

#[test]
fn context_helpers_report_the_representable_safe_method_set() {
    assert!(SameSiteRequestContext::cross_site_navigation(Method::Get).is_safe_method());
    assert!(SameSiteRequestContext::cross_site_navigation(Method::Head).is_safe_method());
    assert!(!SameSiteRequestContext::cross_site_navigation(Method::Post).is_safe_method());
}
