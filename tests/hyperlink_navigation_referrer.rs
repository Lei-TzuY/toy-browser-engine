use std::cell::RefCell;
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
struct HyperlinkLoader {
    seen: Mutex<Vec<FetchRequest>>,
}

impl HyperlinkLoader {
    fn requests(&self) -> Vec<FetchRequest> {
        self.seen.lock().unwrap().clone()
    }
}

impl ResourceLoader for HyperlinkLoader {
    fn load(&self, target: &Url) -> Result<Resource, LoadError> {
        Err(LoadError::NotFound(target.to_string()))
    }

    fn fetch_once(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        self.seen.lock().unwrap().push(request.clone());

        if request.url.path() == "/redirect" {
            let mut response = FetchResponse::synthetic(
                request.url.clone(),
                302,
                Some("text/plain"),
                Vec::new(),
            );
            response
                .headers
                .append_raw("location", "https://target.test/final");
            response
                .headers
                .append_raw("referrer-policy", "no-referrer");
            return Ok(response);
        }

        Ok(FetchResponse::synthetic(
            request.url.clone(),
            200,
            Some("text/html"),
            b"<p>ok</p>".to_vec(),
        ))
    }
}

fn navigation(loader: Arc<HyperlinkLoader>) -> NavigationNetwork {
    let clock = Rc::new(ManualClock::new());
    let jar = Rc::new(RefCell::new(CookieJar::with_clock(clock.clone())));
    let hsts = Rc::new(RefCell::new(HstsCache::new()));
    NavigationNetwork::new(loader, jar, hsts, clock)
}

fn top_level_cross_site() -> SameSiteRequestContext {
    SameSiteRequestContext::new(false, true, Method::Get)
}

#[test]
fn hyperlink_referrerpolicy_controls_the_first_transport_request() {
    let loader = Arc::new(HyperlinkLoader::default());
    let network = navigation(loader.clone());
    let document = DocumentReferrerContext::new(
        Some(url("https://source.test/private/page?q=1#fragment")),
        ReferrerPolicy::StrictOriginWhenCrossOrigin,
    );
    let request = FetchRequest::get(url("https://target.test/final"));

    let (response, _) = document
        .fetch_hyperlink_navigation(
            &network,
            &request,
            top_level_cross_site(),
            Some("unsafe-url"),
            None,
        )
        .expect("hyperlink navigation succeeds");

    assert_eq!(response.status, 200);
    let requests = loader.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].headers.get("referer").as_deref(),
        Some("https://source.test/private/page?q=1")
    );
}

#[test]
fn noreferrer_overrides_an_explicit_unsafe_url_policy() {
    let loader = Arc::new(HyperlinkLoader::default());
    let network = navigation(loader.clone());
    let document = DocumentReferrerContext::new(
        Some(url("https://source.test/private/page?q=1")),
        ReferrerPolicy::UnsafeUrl,
    );
    let request = FetchRequest::get(url("https://target.test/final"));

    document
        .fetch_hyperlink_navigation(
            &network,
            &request,
            top_level_cross_site(),
            Some("unsafe-url"),
            Some("noopener noreferrer"),
        )
        .expect("noreferrer navigation succeeds");

    let requests = loader.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].headers.get("referer").is_none());
}

#[test]
fn redirect_response_can_tighten_the_hyperlink_policy_for_later_hops() {
    let loader = Arc::new(HyperlinkLoader::default());
    let network = navigation(loader.clone());
    let document = DocumentReferrerContext::new(
        Some(url("https://source.test/private/page?q=1")),
        ReferrerPolicy::Origin,
    );
    let request = FetchRequest::get(url("https://target.test/redirect"));

    let (response, next_document) = document
        .fetch_hyperlink_navigation(
            &network,
            &request,
            top_level_cross_site(),
            Some("unsafe-url"),
            None,
        )
        .expect("redirecting hyperlink navigation succeeds");

    assert!(response.redirected);
    let requests = loader.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].headers.get("referer").as_deref(),
        Some("https://source.test/private/page?q=1")
    );
    assert!(
        requests[1].headers.get("referer").is_none(),
        "the redirect response's no-referrer policy must govern the next hop"
    );

    assert_eq!(
        next_document.policy(),
        ReferrerPolicy::StrictOriginWhenCrossOrigin,
        "an intermediate redirect policy must not leak into the committed final document"
    );
}

#[test]
fn hyperlink_override_is_scoped_to_one_navigation() {
    let loader = Arc::new(HyperlinkLoader::default());
    let network = navigation(loader.clone());
    let document = DocumentReferrerContext::new(
        Some(url("https://source.test/private/page?q=1")),
        ReferrerPolicy::Origin,
    );

    document
        .fetch_hyperlink_navigation(
            &network,
            &FetchRequest::get(url("https://target.test/final")),
            top_level_cross_site(),
            Some("no-referrer"),
            None,
        )
        .expect("first hyperlink succeeds");

    document
        .fetch_navigation(
            &network,
            &FetchRequest::get(url("https://another.test/final")),
            top_level_cross_site(),
        )
        .expect("later ordinary navigation succeeds");

    let requests = loader.requests();
    assert!(requests[0].headers.get("referer").is_none());
    assert_eq!(
        requests[1].headers.get("referer").as_deref(),
        Some("https://source.test/"),
        "the source document must retain its own policy after a one-off hyperlink override"
    );
}
