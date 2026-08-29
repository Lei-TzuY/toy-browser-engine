use std::rc::Rc;
use std::sync::{Arc, Mutex};

use browser_engine::cookie::CookieJar;
use browser_engine::cookie_same_site::SameSiteRequestContext;
use browser_engine::eventloop::ManualClock;
use browser_engine::hsts::HstsCache;
use browser_engine::net::{
    FetchError, FetchRequest, FetchResponse, LoadError, Method, Resource, ResourceLoader, Url,
};
use browser_engine::{DocumentReferrerContext, NavigationNetwork, ReferrerPolicy};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

#[derive(Default)]
struct RecordingLoader {
    seen: Mutex<Vec<FetchRequest>>,
}

impl RecordingLoader {
    fn requests(&self) -> Vec<FetchRequest> {
        self.seen.lock().unwrap().clone()
    }
}

impl ResourceLoader for RecordingLoader {
    fn load(&self, target: &Url) -> Result<Resource, LoadError> {
        Err(LoadError::NotFound(target.to_string()))
    }

    fn fetch_once(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        self.seen.lock().unwrap().push(request.clone());

        if request.url.host() == "cdn.test" && request.url.path() == "/redirect.js" {
            let mut response = FetchResponse::synthetic(
                request.url.clone(),
                302,
                Some("text/plain"),
                b"redirect".to_vec(),
            );
            response
                .headers
                .append_raw("location", "https://static.test/final.js");
            response
                .headers
                .append_raw("referrer-policy", "no-referrer");
            return Ok(response);
        }

        Ok(FetchResponse::synthetic(
            request.url.clone(),
            200,
            Some("application/javascript"),
            b"ok".to_vec(),
        ))
    }
}

fn network(loader: Arc<RecordingLoader>) -> NavigationNetwork {
    let clock = Rc::new(ManualClock::new());
    let jar = Rc::new(std::cell::RefCell::new(CookieJar::with_clock(clock.clone())));
    let hsts = Rc::new(std::cell::RefCell::new(HstsCache::new()));
    NavigationNetwork::new(loader, jar, hsts, clock)
}

fn subresource_context(method: Method) -> SameSiteRequestContext {
    SameSiteRequestContext::new(false, false, method)
}

#[test]
fn element_override_reaches_actual_subresource_request() {
    let loader = Arc::new(RecordingLoader::default());
    let network = network(loader.clone());
    let document = DocumentReferrerContext::new(
        Some(url("https://page.test/private/gallery?album=1#secret")),
        ReferrerPolicy::NoReferrer,
    );
    let mut request = FetchRequest::get(url("https://cdn.test/app.js"));
    request
        .headers
        .insert_raw("referer", "https://attacker.invalid/forged");

    let response = document
        .fetch_subresource(
            &network,
            &request,
            subresource_context(Method::Get),
            Some("unsafe-url"),
        )
        .expect("subresource fetch succeeds");

    assert_eq!(response.status, 200);
    let requests = loader.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].headers.get("referer").as_deref(),
        Some("https://page.test/private/gallery?album=1")
    );
    assert_eq!(document.policy(), ReferrerPolicy::NoReferrer);
}

#[test]
fn redirect_response_can_tighten_element_selected_policy() {
    let loader = Arc::new(RecordingLoader::default());
    let network = network(loader.clone());
    let document = DocumentReferrerContext::new(
        Some(url("https://page.test/private/app?q=1#secret")),
        ReferrerPolicy::Origin,
    );

    let response = document
        .fetch_subresource(
            &network,
            &FetchRequest::get(url("https://cdn.test/redirect.js")),
            subresource_context(Method::Get),
            Some("unsafe-url"),
        )
        .expect("redirected subresource fetch succeeds");

    assert_eq!(response.status, 200);
    assert!(response.redirected);
    let requests = loader.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].headers.get("referer").as_deref(),
        Some("https://page.test/private/app?q=1")
    );
    assert!(
        requests[1].headers.get("referer").is_none(),
        "redirect response no-referrer must suppress the following subresource hop"
    );
}

#[test]
fn invalid_element_policy_inherits_document_policy_on_transport() {
    let loader = Arc::new(RecordingLoader::default());
    let network = network(loader.clone());
    let document = DocumentReferrerContext::new(
        Some(url("https://page.test/private/app?q=1#secret")),
        ReferrerPolicy::Origin,
    );

    document
        .fetch_subresource(
            &network,
            &FetchRequest::get(url("https://cdn.test/app.js")),
            subresource_context(Method::Get),
            Some(" future-policy "),
        )
        .expect("subresource fetch succeeds");

    let requests = loader.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].headers.get("referer").as_deref(),
        Some("https://page.test/")
    );
    assert_eq!(document.policy(), ReferrerPolicy::Origin);
}
