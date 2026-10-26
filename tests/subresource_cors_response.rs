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

#[derive(Clone, Copy)]
enum CorsReply {
    None,
    Wildcard,
    Exact,
    ExactWithCredentials,
}

struct CorsLoader {
    reply: CorsReply,
}

impl ResourceLoader for CorsLoader {
    fn load(&self, target: &Url) -> Result<Resource, LoadError> {
        Err(LoadError::NotFound(target.to_string()))
    }

    fn fetch_once(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        let mut response = FetchResponse::synthetic(
            request.url.clone(),
            200,
            Some("application/javascript"),
            b"ok".to_vec(),
        );
        match self.reply {
            CorsReply::None => {}
            CorsReply::Wildcard => {
                response.headers.insert_raw("access-control-allow-origin", "*");
            }
            CorsReply::Exact => {
                response
                    .headers
                    .insert_raw("access-control-allow-origin", "https://page.test");
            }
            CorsReply::ExactWithCredentials => {
                response
                    .headers
                    .insert_raw("access-control-allow-origin", "https://page.test");
                response
                    .headers
                    .insert_raw("access-control-allow-credentials", "true");
            }
        }
        Ok(response)
    }
}

fn network(reply: CorsReply) -> NavigationNetwork {
    let clock = Rc::new(ManualClock::new());
    let jar = Rc::new(std::cell::RefCell::new(CookieJar::with_clock(clock.clone())));
    let hsts = Rc::new(std::cell::RefCell::new(HstsCache::new()));
    NavigationNetwork::new(Arc::new(CorsLoader { reply }), jar, hsts, clock)
}

fn subresource_context() -> SameSiteRequestContext {
    SameSiteRequestContext::new(false, false, Method::Get)
}

fn document() -> DocumentReferrerContext {
    DocumentReferrerContext::from_url(url("https://page.test/index.html"))
}

#[test]
fn anonymous_cross_origin_resource_requires_cors_response_permission() {
    let error = document()
        .fetch_subresource_with_cors(
            &network(CorsReply::None),
            &FetchRequest::get(url("https://cdn.test/app.js")),
            subresource_context(),
            None,
            Some("anonymous"),
        )
        .expect_err("missing ACAO must block a CORS-enabled cross-origin response");

    assert!(matches!(error, FetchError::Blocked(message) if message.contains("CORS")));
}

#[test]
fn anonymous_cross_origin_resource_accepts_wildcard() {
    let response = document()
        .fetch_subresource_with_cors(
            &network(CorsReply::Wildcard),
            &FetchRequest::get(url("https://cdn.test/app.js")),
            subresource_context(),
            None,
            Some("anonymous"),
        )
        .expect("wildcard ACAO permits anonymous CORS");

    assert_eq!(response.status, 200);
}

#[test]
fn credentialed_resource_rejects_wildcard_even_with_allow_credentials() {
    let error = document()
        .fetch_subresource_with_cors(
            &network(CorsReply::Wildcard),
            &FetchRequest::get(url("https://cdn.test/app.js")),
            subresource_context(),
            None,
            Some("use-credentials"),
        )
        .expect_err("credentialed CORS cannot use wildcard ACAO");

    assert!(matches!(error, FetchError::Blocked(_)));
}

#[test]
fn credentialed_resource_requires_allow_credentials_true() {
    let error = document()
        .fetch_subresource_with_cors(
            &network(CorsReply::Exact),
            &FetchRequest::get(url("https://cdn.test/app.js")),
            subresource_context(),
            None,
            Some("use-credentials"),
        )
        .expect_err("exact ACAO alone is not enough for credentialed CORS");

    assert!(matches!(error, FetchError::Blocked(_)));
}

#[test]
fn credentialed_resource_accepts_exact_origin_plus_credentials() {
    let response = document()
        .fetch_subresource_with_cors(
            &network(CorsReply::ExactWithCredentials),
            &FetchRequest::get(url("https://cdn.test/app.js")),
            subresource_context(),
            None,
            Some("use-credentials"),
        )
        .expect("credentialed CORS response is valid");

    assert_eq!(response.status, 200);
}

#[test]
fn missing_crossorigin_preserves_existing_no_cors_fetch_path() {
    let response = document()
        .fetch_subresource_with_cors(
            &network(CorsReply::None),
            &FetchRequest::get(url("https://cdn.test/app.js")),
            subresource_context(),
            None,
            None,
        )
        .expect("no-CORS path does not require ACAO");

    assert_eq!(response.status, 200);
}

#[test]
fn same_origin_cors_resource_does_not_require_acao() {
    let response = document()
        .fetch_subresource_with_cors(
            &network(CorsReply::None),
            &FetchRequest::get(url("https://page.test/app.js")),
            SameSiteRequestContext::new(true, false, Method::Get),
            None,
            Some("anonymous"),
        )
        .expect("same-origin CORS-mode request is not subject to cross-origin response check");

    assert_eq!(response.status, 200);
}
