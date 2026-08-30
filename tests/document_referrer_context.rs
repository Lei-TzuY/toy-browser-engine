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
    requests: Mutex<Vec<FetchRequest>>,
}

impl RecordingLoader {
    fn requests(&self) -> Vec<FetchRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl ResourceLoader for RecordingLoader {
    fn load(&self, target: &Url) -> Result<Resource, LoadError> {
        Err(LoadError::NotFound(target.to_string()))
    }

    fn fetch_once(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        self.requests.lock().unwrap().push(request.clone());
        match (request.url.host(), request.url.path()) {
            ("target.test", "/start") => {
                let mut response = FetchResponse::synthetic(
                    request.url.clone(),
                    302,
                    Some("text/plain"),
                    b"redirect".to_vec(),
                );
                response
                    .headers
                    .append_raw("location", "https://source.test/landing");
                response.headers.append_raw("referrer-policy", "unsafe-url");
                Ok(response)
            }
            ("source.test", "/landing") => {
                let mut response = FetchResponse::synthetic(
                    request.url.clone(),
                    200,
                    Some("text/html"),
                    b"<p>landed</p>".to_vec(),
                );
                response
                    .headers
                    .append_raw("referrer-policy", "no-referrer");
                Ok(response)
            }
            ("source.test", "/next") => Ok(FetchResponse::synthetic(
                request.url.clone(),
                200,
                Some("text/html"),
                b"<p>next</p>".to_vec(),
            )),
            _ => Ok(FetchResponse::synthetic(
                request.url.clone(),
                404,
                Some("text/plain"),
                b"not found".to_vec(),
            )),
        }
    }
}

fn navigation(loader: Arc<RecordingLoader>) -> NavigationNetwork {
    let clock = Rc::new(ManualClock::new());
    let jar = Rc::new(std::cell::RefCell::new(CookieJar::with_clock(
        clock.clone(),
    )));
    let hsts = Rc::new(std::cell::RefCell::new(HstsCache::new()));
    NavigationNetwork::new(loader, jar, hsts, clock)
}

#[test]
fn committed_final_policy_is_used_by_the_following_navigation() {
    let loader = Arc::new(RecordingLoader::default());
    let network = navigation(loader.clone());
    let current = DocumentReferrerContext::new(
        Some(url("https://source.test/private/page?q=1#secret")),
        ReferrerPolicy::StrictOriginWhenCrossOrigin,
    );

    let (response, committed) = current
        .fetch_navigation(
            &network,
            &FetchRequest::get(url("https://target.test/start")),
            SameSiteRequestContext::new(false, true, Method::Get),
        )
        .expect("redirected navigation succeeds");

    assert_eq!(response.url.to_string(), "https://source.test/landing");
    assert!(response.redirected);
    assert_eq!(committed.policy(), ReferrerPolicy::NoReferrer);
    assert_eq!(
        committed.source().map(ToString::to_string).as_deref(),
        Some("https://source.test/landing")
    );

    let requests = loader.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].headers.get("referer").as_deref(),
        Some("https://source.test/"),
        "the first cross-origin hop must use the outgoing document's default origin-only referrer"
    );
    assert_eq!(
        requests[1].headers.get("referer").as_deref(),
        Some("https://source.test/private/page?q=1"),
        "the redirect's unsafe-url policy must still recompute from the stable outgoing document source"
    );

    let (_next_response, next_context) = committed
        .fetch_navigation(
            &network,
            &FetchRequest::get(url("https://source.test/next")),
            SameSiteRequestContext::same_site(Method::Get),
        )
        .expect("following navigation succeeds");

    let requests = loader.requests();
    assert_eq!(requests.len(), 3);
    assert!(
        requests[2].headers.get("referer").is_none(),
        "the final document's no-referrer policy must govern the next navigation"
    );
    assert_eq!(next_context.policy(), ReferrerPolicy::default());
}

#[test]
fn independent_navigation_states_do_not_mutate_the_outgoing_document() {
    let loader = Arc::new(RecordingLoader::default());
    let network = navigation(loader);
    let current = DocumentReferrerContext::from_url(url("https://source.test/page"));

    let (_response, committed) = current
        .fetch_navigation(
            &network,
            &FetchRequest::get(url("https://source.test/next")),
            SameSiteRequestContext::same_site(Method::Get),
        )
        .expect("navigation succeeds");

    assert_eq!(
        current.source().unwrap().to_string(),
        "https://source.test/page"
    );
    assert_eq!(current.policy(), ReferrerPolicy::default());
    assert_eq!(
        committed.source().unwrap().to_string(),
        "https://source.test/next"
    );
}
