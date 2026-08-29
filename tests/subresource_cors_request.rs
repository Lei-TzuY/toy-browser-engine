use std::rc::Rc;
use std::sync::{Arc, Mutex};

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

struct RecordingCorsLoader {
    seen: Arc<Mutex<Vec<FetchRequest>>>,
    redirect: bool,
}

impl ResourceLoader for RecordingCorsLoader {
    fn load(&self, target: &Url) -> Result<Resource, LoadError> {
        Err(LoadError::NotFound(target.to_string()))
    }

    fn fetch_once(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        self.seen.lock().unwrap().push(request.clone());

        if self.redirect && request.url.to_string() == "https://cdn.test/start.js" {
            let mut response = FetchResponse::synthetic(
                request.url.clone(),
                302,
                Some("text/plain"),
                Vec::new(),
            );
            response.headers.insert_raw("location", "https://static.test/final.js");
            return Ok(response);
        }

        let mut response = FetchResponse::synthetic(
            request.url.clone(),
            200,
            Some("application/javascript"),
            b"ok".to_vec(),
        );
        response
            .headers
            .insert_raw("access-control-allow-origin", "https://page.test");
        Ok(response)
    }
}

fn network(redirect: bool) -> (NavigationNetwork, Arc<Mutex<Vec<FetchRequest>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let loader = RecordingCorsLoader {
        seen: seen.clone(),
        redirect,
    };
    let clock = Rc::new(ManualClock::new());
    let jar = Rc::new(std::cell::RefCell::new(CookieJar::with_clock(clock.clone())));
    let hsts = Rc::new(std::cell::RefCell::new(HstsCache::new()));
    (
        NavigationNetwork::new(Arc::new(loader), jar, hsts, clock),
        seen,
    )
}

fn context() -> SameSiteRequestContext {
    SameSiteRequestContext::new(false, false, Method::Get)
}

#[test]
fn cors_subresource_sends_browser_owned_origin_instead_of_caller_value() {
    let (network, seen) = network(false);
    let document = DocumentReferrerContext::from_url(url("https://page.test/private/index.html"));
    let mut request = FetchRequest::get(url("https://cdn.test/app.js"));
    request.headers.insert_raw("origin", "https://forged.test");

    document
        .fetch_cors_subresource(
            &network,
            &request,
            context(),
            None,
            Some("anonymous"),
        )
        .expect("exact ACAO permits request");

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].headers.get("origin").as_deref(), Some("https://page.test"));
}

#[test]
fn cors_origin_survives_redirect_chain_as_stable_document_origin() {
    let (network, seen) = network(true);
    let document = DocumentReferrerContext::from_url(url("https://page.test/index.html"));

    let response = document
        .fetch_cors_subresource(
            &network,
            &FetchRequest::get(url("https://cdn.test/start.js")),
            context(),
            None,
            Some("anonymous"),
        )
        .expect("redirected CORS request should pass final ACAO");

    assert!(response.redirected);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].headers.get("origin").as_deref(), Some("https://page.test"));
    assert_eq!(seen[1].headers.get("origin").as_deref(), Some("https://page.test"));
}

#[test]
fn no_cors_subresource_does_not_invent_origin_header() {
    let (network, seen) = network(false);
    let document = DocumentReferrerContext::from_url(url("https://page.test/index.html"));

    document
        .fetch_cors_subresource(
            &network,
            &FetchRequest::get(url("https://cdn.test/app.js")),
            context(),
            None,
            None,
        )
        .expect("no-CORS path remains compatible");

    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].headers.get("origin"), None);
}
