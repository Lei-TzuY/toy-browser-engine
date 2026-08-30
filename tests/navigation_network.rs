use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use browser_engine::cookie::CookieJar;
use browser_engine::eventloop::{Clock, ManualClock};
use browser_engine::hsts::HstsCache;
use browser_engine::net::{
    FetchError, FetchRequest, FetchResponse, HeaderMap, LoadError, Method, Resource,
    ResourceLoader, Url,
};
use browser_engine::{NavigationNetwork, SameSiteRequestContext};

struct RecordingLoader {
    requests: Mutex<Vec<FetchRequest>>,
    responses: Mutex<VecDeque<FetchResponse>>,
}

impl RecordingLoader {
    fn new(responses: Vec<FetchResponse>) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            responses: Mutex::new(responses.into()),
        }
    }

    fn requests(&self) -> Vec<FetchRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl ResourceLoader for RecordingLoader {
    fn load(&self, url: &Url) -> Result<Resource, LoadError> {
        Err(LoadError::NotFound(url.to_string()))
    }

    fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        self.requests.lock().unwrap().push(request.clone());
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| FetchError::Io("no queued response".into()))
    }
}

fn url(input: &str) -> Url {
    Url::parse(input).unwrap()
}

fn response(url: &str) -> FetchResponse {
    FetchResponse::synthetic(
        url::parse(url).unwrap(),
        200,
        Some("text/html"),
        b"ok".to_vec(),
    )
}

mod url {
    use browser_engine::net::Url;

    pub fn parse(input: &str) -> Result<Url, browser_engine::net::UrlError> {
        Url::parse(input)
    }
}

fn navigation(
    loader: Arc<RecordingLoader>,
) -> (
    NavigationNetwork,
    Rc<RefCell<CookieJar>>,
    Rc<RefCell<HstsCache>>,
    Rc<ManualClock>,
) {
    let clock = Rc::new(ManualClock::new());
    let jar = Rc::new(RefCell::new(CookieJar::with_clock(clock.clone())));
    let hsts = Rc::new(RefCell::new(HstsCache::new()));
    let policy = NavigationNetwork::new(
        loader as Arc<dyn ResourceLoader>,
        jar.clone(),
        hsts.clone(),
        clock.clone() as Rc<dyn Clock>,
    );
    (policy, jar, hsts, clock)
}

#[test]
fn hsts_upgrade_precedes_samesite_and_secure_cookie_selection() {
    let loader = Arc::new(RecordingLoader::new(vec![response(
        "https://example.test/next",
    )]));
    let (navigation, jar, hsts, _) = navigation(loader.clone());

    hsts.borrow_mut()
        .observe_response(&url("https://example.test/"), "max-age=60", 0);
    for cookie in [
        "strict=s; Path=/; Secure; SameSite=Strict",
        "lax=l; Path=/; Secure; SameSite=Lax",
        "none=n; Path=/; Secure; SameSite=None",
    ] {
        assert!(jar
            .borrow_mut()
            .store_set_cookie(cookie, &url("https://example.test/"), 0));
    }

    let mut headers = HeaderMap::new();
    headers.insert_raw("cookie", "forged=page");
    let request = FetchRequest::new(url("http://example.test/next"), Method::Get, headers, None);

    navigation
        .fetch(
            &request,
            SameSiteRequestContext::cross_site_navigation(Method::Get),
        )
        .unwrap();

    let requests = loader.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.to_string(), "https://example.test/next");
    let cookie = requests[0].headers.get("cookie").unwrap();
    assert!(cookie.contains("lax=l"), "{cookie}");
    assert!(cookie.contains("none=n"), "{cookie}");
    assert!(!cookie.contains("strict=s"), "{cookie}");
    assert!(!cookie.contains("forged=page"), "{cookie}");
}

#[test]
fn cross_site_post_navigation_sends_only_samesite_none() {
    let loader = Arc::new(RecordingLoader::new(vec![response(
        "https://example.test/submit",
    )]));
    let (navigation, jar, _, _) = navigation(loader.clone());

    for cookie in [
        "strict=s; Path=/; Secure; SameSite=Strict",
        "lax=l; Path=/; Secure; SameSite=Lax",
        "none=n; Path=/; Secure; SameSite=None",
    ] {
        assert!(jar
            .borrow_mut()
            .store_set_cookie(cookie, &url("https://example.test/"), 0));
    }

    let request = FetchRequest::new(
        url("https://example.test/submit"),
        Method::Post,
        HeaderMap::new(),
        Some(b"x=1".to_vec()),
    );
    navigation
        .fetch(
            &request,
            SameSiteRequestContext::cross_site_navigation(Method::Post),
        )
        .unwrap();

    let requests = loader.requests();
    let cookie = requests[0].headers.get("cookie").unwrap();
    assert_eq!(cookie, "none=n");
}

#[test]
fn response_state_is_absorbed_and_set_cookie_is_hidden() {
    let mut first = response("https://example.test/login");
    first
        .headers
        .append_raw("set-cookie", "auth=one; Path=/; Secure; SameSite=Lax");
    first
        .headers
        .append_raw("set-cookie", "prefs=two; Path=/; Secure; SameSite=None");
    first
        .headers
        .append_raw("strict-transport-security", "max-age=60");
    first
        .headers
        .append_raw("strict-transport-security", "max-age=0");

    let second = response("https://example.test/account");
    let loader = Arc::new(RecordingLoader::new(vec![first, second]));
    let (navigation, jar, hsts, _) = navigation(loader.clone());

    let visible = navigation
        .get(
            &url("http://example.test/login"),
            SameSiteRequestContext::same_site(Method::Get),
        )
        .unwrap();

    assert!(!visible.headers.has("set-cookie"));
    assert_eq!(jar.borrow().len(), 2);
    assert!(hsts.borrow().is_known_host("example.test", 0));

    navigation
        .get(
            &url("http://example.test/account"),
            SameSiteRequestContext::same_site(Method::Get),
        )
        .unwrap();

    let requests = loader.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].url.to_string(), "http://example.test/login");
    assert_eq!(requests[1].url.to_string(), "https://example.test/account");
    let cookie = requests[1].headers.get("cookie").unwrap();
    assert!(cookie.contains("auth=one"), "{cookie}");
    assert!(cookie.contains("prefs=two"), "{cookie}");
}
