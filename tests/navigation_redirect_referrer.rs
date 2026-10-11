use std::rc::Rc;
use std::sync::{Arc, Mutex};

use browser_engine::cookie::CookieJar;
use browser_engine::cookie_same_site::SameSiteRequestContext;
use browser_engine::eventloop::ManualClock;
use browser_engine::hsts::HstsCache;
use browser_engine::net::{
    FetchError, FetchRequest, FetchResponse, LoadError, Method, Resource, ResourceLoader, Url,
};
use browser_engine::{NavigationNetwork, RedirectReferrerState, ReferrerPolicy};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

#[derive(Default)]
struct ReferrerLoader {
    seen: Mutex<Vec<FetchRequest>>,
}

impl ReferrerLoader {
    fn requests(&self) -> Vec<FetchRequest> {
        self.seen.lock().unwrap().clone()
    }

    fn redirect(request: &FetchRequest, location: &str, policy: &str) -> FetchResponse {
        let mut response = FetchResponse::synthetic(
            request.url.clone(),
            302,
            Some("text/plain"),
            b"redirect".to_vec(),
        );
        response.headers.append_raw("location", location);
        response.headers.append_raw("referrer-policy", policy);
        response
    }
}

impl ResourceLoader for ReferrerLoader {
    fn load(&self, target: &Url) -> Result<Resource, LoadError> {
        Err(LoadError::NotFound(target.to_string()))
    }

    fn fetch_once(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        self.seen.lock().unwrap().push(request.clone());
        match (request.url.host(), request.url.path()) {
            ("target.test", "/start") => Ok(Self::redirect(
                request,
                "https://source.test/final",
                "unsafe-url",
            )),
            ("target.test", "/suppress") => Ok(Self::redirect(
                request,
                "https://source.test/final",
                "no-referrer",
            )),
            ("source.test", "/final") => Ok(FetchResponse::synthetic(
                request.url.clone(),
                200,
                Some("text/plain"),
                b"done".to_vec(),
            )),
            ("hsts.test", "/start") => {
                let mut response = Self::redirect(
                    request,
                    "http://hsts.test/final",
                    "strict-origin",
                );
                response
                    .headers
                    .append_raw("strict-transport-security", "max-age=60");
                Ok(response)
            }
            ("hsts.test", "/final") => Ok(FetchResponse::synthetic(
                request.url.clone(),
                200,
                Some("text/plain"),
                b"hsts-done".to_vec(),
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

fn navigation(loader: Arc<ReferrerLoader>) -> NavigationNetwork {
    let clock = Rc::new(ManualClock::new());
    let jar = Rc::new(std::cell::RefCell::new(CookieJar::with_clock(clock.clone())));
    let hsts = Rc::new(std::cell::RefCell::new(HstsCache::new()));
    NavigationNetwork::new(loader, jar, hsts, clock)
}

#[test]
fn redirect_policy_recomputes_referer_from_original_source() {
    let loader = Arc::new(ReferrerLoader::default());
    let network = navigation(loader.clone());
    let source = url("https://source.test/private/page?q=1#secret");
    let state = RedirectReferrerState::new(
        Some(source),
        ReferrerPolicy::StrictOriginWhenCrossOrigin,
    );

    let response = network
        .get_with_referrer(
            &url("https://target.test/start"),
            SameSiteRequestContext::new(false, true, Method::Get),
            state,
        )
        .expect("redirect chain succeeds");

    assert_eq!(response.status, 200);
    assert!(response.redirected);
    let requests = loader.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].headers.get("referer").as_deref(),
        Some("https://source.test/"),
        "the first cross-origin hop uses the default origin-only serialization"
    );
    assert_eq!(
        requests[1].headers.get("referer").as_deref(),
        Some("https://source.test/private/page?q=1"),
        "unsafe-url on the redirect must recompute from the original source, not the previous origin-only header"
    );
}

#[test]
fn redirect_no_referrer_policy_suppresses_the_next_hop() {
    let loader = Arc::new(ReferrerLoader::default());
    let network = navigation(loader.clone());
    let state = RedirectReferrerState::from_source(url(
        "https://source.test/private/page?q=1#secret",
    ));

    network
        .get_with_referrer(
            &url("https://target.test/suppress"),
            SameSiteRequestContext::new(false, true, Method::Get),
            state,
        )
        .expect("redirect chain succeeds");

    let requests = loader.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].headers.get("referer").is_none());
}

#[test]
fn hsts_upgrade_happens_before_redirect_referer_computation() {
    let loader = Arc::new(ReferrerLoader::default());
    let network = navigation(loader.clone());
    let state = RedirectReferrerState::new(
        Some(url("https://source.test/private")),
        ReferrerPolicy::StrictOrigin,
    );

    network
        .get_with_referrer(
            &url("https://hsts.test/start"),
            SameSiteRequestContext::new(false, true, Method::Get),
            state,
        )
        .expect("redirect chain succeeds");

    let requests = loader.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].url.to_string(), "https://hsts.test/final");
    assert_eq!(
        requests[1].headers.get("referer").as_deref(),
        Some("https://source.test/"),
        "strict-origin must observe the HSTS-upgraded HTTPS target rather than treating the authored HTTP Location as a downgrade"
    );
}

#[test]
fn legacy_fetch_path_does_not_invent_referrer_state() {
    let loader = Arc::new(ReferrerLoader::default());
    let network = navigation(loader.clone());
    let mut request = FetchRequest::get(url("https://target.test/start"));
    request
        .headers
        .insert_raw("referer", "https://caller.test/already-computed");

    network
        .fetch(
            &request,
            SameSiteRequestContext::new(false, true, Method::Get),
        )
        .expect("legacy redirect chain succeeds");

    let requests = loader.requests();
    assert_eq!(
        requests[0].headers.get("referer").as_deref(),
        Some("https://caller.test/already-computed")
    );
    assert!(
        requests[1].headers.get("referer").is_none(),
        "RedirectPlanner still strips stale Referer when no browser-owned source state was supplied"
    );
}
