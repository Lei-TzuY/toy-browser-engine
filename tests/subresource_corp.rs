use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use browser_engine::cookie::CookieJar;
use browser_engine::cookie_same_site::SameSiteRequestContext;
use browser_engine::eventloop::ManualClock;
use browser_engine::hsts::HstsCache;
use browser_engine::net::{
    FetchError, FetchRequest, FetchResponse, LoadError, Method, Resource, ResourceLoader, Url,
};
use browser_engine::{DocumentReferrerContext, NavigationNetwork};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

struct CorpLoader {
    policy: Option<&'static str>,
    allow_origin: bool,
}

impl ResourceLoader for CorpLoader {
    fn load(&self, target: &Url) -> Result<Resource, LoadError> {
        Err(LoadError::NotFound(target.to_string()))
    }

    fn fetch_once(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        let mut response = FetchResponse::synthetic(
            request.url.clone(),
            200,
            Some("image/png"),
            b"resource".to_vec(),
        );
        if let Some(policy) = self.policy {
            response
                .headers
                .insert_raw("cross-origin-resource-policy", policy);
        }
        if self.allow_origin {
            response
                .headers
                .insert_raw("access-control-allow-origin", "https://page.test");
        }
        Ok(response)
    }
}

fn network(policy: Option<&'static str>, allow_origin: bool) -> NavigationNetwork {
    let clock = Rc::new(ManualClock::new());
    let jar = Rc::new(RefCell::new(CookieJar::with_clock(clock.clone())));
    let hsts = Rc::new(RefCell::new(HstsCache::new()));
    NavigationNetwork::new(
        Arc::new(CorpLoader {
            policy,
            allow_origin,
        }),
        jar,
        hsts,
        clock,
    )
}

fn context(same_site: bool) -> SameSiteRequestContext {
    SameSiteRequestContext::new(same_site, false, Method::Get)
}

#[test]
fn same_origin_corp_blocks_cross_origin_no_cors_element_load() {
    let document = DocumentReferrerContext::from_url(url("https://page.test/index.html"));
    let error = document
        .fetch_subresource_with_cors_credentials(
            &network(Some("same-origin"), false),
            &FetchRequest::get(url("https://cdn.test/private.png")),
            context(false),
            None,
            None,
        )
        .expect_err("cross-origin no-CORS body must not be exposed");

    assert!(matches!(error, FetchError::Blocked(_)));
    assert!(error.to_string().contains("Cross-Origin-Resource-Policy same-origin"));
}

#[test]
fn same_origin_corp_allows_same_origin_no_cors_element_load() {
    let document = DocumentReferrerContext::from_url(url("https://page.test/index.html"));
    let response = document
        .fetch_subresource_with_cors_credentials(
            &network(Some("same-origin"), false),
            &FetchRequest::get(url("https://page.test/private.png")),
            context(true),
            None,
            None,
        )
        .expect("same-origin resource is permitted");

    assert_eq!(response.status, 200);
}

#[test]
fn same_site_corp_uses_browser_site_classification() {
    let document = DocumentReferrerContext::from_url(url("https://page.test/index.html"));
    let response = document
        .fetch_subresource_with_cors_credentials(
            &network(Some("same-site"), false),
            &FetchRequest::get(url("https://static.page.test/image.png")),
            context(true),
            None,
            None,
        )
        .expect("same-site classification permits the resource");

    assert_eq!(response.status, 200);
}

#[test]
fn cross_origin_corp_explicitly_allows_cross_origin_no_cors_load() {
    let document = DocumentReferrerContext::from_url(url("https://page.test/index.html"));
    let response = document
        .fetch_subresource_with_cors_credentials(
            &network(Some("cross-origin"), false),
            &FetchRequest::get(url("https://cdn.test/public.png")),
            context(false),
            None,
            None,
        )
        .expect("cross-origin policy permits the resource");

    assert_eq!(response.status, 200);
}

#[test]
fn successful_cors_request_is_not_rejected_by_no_cors_corp_gate() {
    let document = DocumentReferrerContext::from_url(url("https://page.test/index.html"));
    let response = document
        .fetch_subresource_with_cors_credentials(
            &network(Some("same-origin"), true),
            &FetchRequest::get(url("https://cdn.test/cors.png")),
            context(false),
            None,
            Some("anonymous"),
        )
        .expect("valid CORS sharing permission governs the CORS request");

    assert_eq!(response.status, 200);
}

#[test]
fn invalid_case_corp_token_is_ignored_per_fetch_grammar() {
    let document = DocumentReferrerContext::from_url(url("https://page.test/index.html"));
    let response = document
        .fetch_subresource_with_cors_credentials(
            &network(Some("Same-Origin"), false),
            &FetchRequest::get(url("https://cdn.test/image.png")),
            context(false),
            None,
            None,
        )
        .expect("case-mismatched token is not a recognized CORP policy");

    assert_eq!(response.status, 200);
}
