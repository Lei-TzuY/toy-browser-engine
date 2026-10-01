use std::rc::Rc;
use std::sync::{Arc, Mutex};

use browser_engine::cookie::CookieJar;
use browser_engine::cookie_same_site::SameSiteRequestContext;
use browser_engine::eventloop::ManualClock;
use browser_engine::hsts::HstsCache;
use browser_engine::net::{
    FetchError, FetchRequest, FetchResponse, LoadError, Method, Resource, ResourceLoader, Url,
};
use browser_engine::NavigationNetwork;

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

#[derive(Default)]
struct ScriptedLoader {
    seen: Mutex<Vec<FetchRequest>>,
}

impl ScriptedLoader {
    fn requests(&self) -> Vec<FetchRequest> {
        self.seen.lock().unwrap().clone()
    }

    fn response(
        request: &FetchRequest,
        status: u16,
        body: &[u8],
        location: Option<&str>,
    ) -> FetchResponse {
        let mut response = FetchResponse::synthetic(
            request.url.clone(),
            status,
            Some("text/plain"),
            body.to_vec(),
        );
        if let Some(location) = location {
            response.headers.append_raw("location", location);
        }
        response
    }
}

impl ResourceLoader for ScriptedLoader {
    fn load(&self, target: &Url) -> Result<Resource, LoadError> {
        Err(LoadError::NotFound(target.to_string()))
    }

    fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        // Navigation redirect orchestration must use fetch_once. Returning a
        // distinctive failure here makes an accidental fallback immediately
        // visible in every regression below.
        Ok(FetchResponse::synthetic(
            request.url.clone(),
            598,
            Some("text/plain"),
            b"legacy-fetch-called".to_vec(),
        ))
    }

    fn fetch_once(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        self.seen.lock().unwrap().push(request.clone());
        match (request.url.host(), request.url.path()) {
            ("example.test", "/cookie-start") => {
                let mut response = Self::response(request, 302, b"redirect", Some("/cookie-next"));
                response
                    .headers
                    .append_raw("set-cookie", "hop=one; Path=/; SameSite=Strict");
                Ok(response)
            }
            ("example.test", "/cookie-next") => {
                Ok(Self::response(request, 200, b"cookie-final", None))
            }
            ("example.test", "/hsts-start") => {
                let mut response = Self::response(
                    request,
                    302,
                    b"redirect",
                    Some("http://example.test/hsts-next"),
                );
                response
                    .headers
                    .append_raw("strict-transport-security", "max-age=60");
                response.headers.append_raw(
                    "set-cookie",
                    "securehop=one; Path=/; Secure; SameSite=Strict",
                );
                Ok(response)
            }
            ("example.test", "/hsts-next") => {
                Ok(Self::response(request, 200, b"hsts-final", None))
            }
            ("example.test", "/cross-start") => Ok(Self::response(
                request,
                302,
                b"redirect",
                Some("http://other.test/cross-final"),
            )),
            ("other.test", "/cross-final") => {
                Ok(Self::response(request, 200, b"cross-final", None))
            }
            ("example.test", "/post-start") => Ok(Self::response(
                request,
                302,
                b"redirect",
                Some("http://other.test/post-final"),
            )),
            ("other.test", "/post-final") => {
                Ok(Self::response(request, 200, b"post-final", None))
            }
            _ => Ok(Self::response(request, 404, b"not found", None)),
        }
    }
}

fn navigation(
    loader: Arc<ScriptedLoader>,
) -> (
    NavigationNetwork,
    browser_engine::cookie_network::CookieJarRef,
) {
    let clock = Rc::new(ManualClock::new());
    let jar = Rc::new(std::cell::RefCell::new(CookieJar::with_clock(clock.clone())));
    let hsts = Rc::new(std::cell::RefCell::new(HstsCache::new()));
    (
        NavigationNetwork::new(loader, jar.clone(), hsts, clock),
        jar,
    )
}

#[test]
fn intermediate_set_cookie_is_sent_on_the_next_same_site_hop() {
    let loader = Arc::new(ScriptedLoader::default());
    let (network, _jar) = navigation(loader.clone());

    let response = network
        .get(
            &url("http://example.test/cookie-start"),
            SameSiteRequestContext::new(true, true, Method::Get),
        )
        .expect("redirect chain succeeds");

    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"cookie-final");
    assert!(response.redirected);
    assert!(response.headers.get("set-cookie").is_none());

    let requests = loader.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].headers.get("cookie").as_deref(),
        Some("hop=one"),
        "the redirect response must update the jar before the next dispatch"
    );
}

#[test]
fn intermediate_sts_upgrades_location_before_secure_cookie_selection() {
    let loader = Arc::new(ScriptedLoader::default());
    let (network, _jar) = navigation(loader.clone());

    let response = network
        .get(
            &url("https://example.test/hsts-start"),
            SameSiteRequestContext::new(true, true, Method::Get),
        )
        .expect("redirect chain succeeds");

    assert_eq!(response.status, 200);
    let requests = loader.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].url.to_string(),
        "https://example.test/hsts-next",
        "learned HSTS must transform the authored HTTP Location before dispatch"
    );
    assert_eq!(
        requests[1].headers.get("cookie").as_deref(),
        Some("securehop=one"),
        "Secure cookie selection must observe the HSTS-effective URL"
    );
}

#[test]
fn a_cross_site_redirect_chain_cannot_regain_strict_cookie_eligibility() {
    let loader = Arc::new(ScriptedLoader::default());
    let (network, jar) = navigation(loader.clone());
    assert!(jar.borrow_mut().store_set_cookie(
        "strict=blocked; Path=/; SameSite=Strict",
        &url("http://other.test/"),
        0,
    ));
    assert!(jar.borrow_mut().store_set_cookie(
        "lax=allowed; Path=/; SameSite=Lax",
        &url("http://other.test/"),
        0,
    ));

    network
        .get(
            &url("http://example.test/cross-start"),
            SameSiteRequestContext::new(true, true, Method::Get),
        )
        .expect("redirect chain succeeds");

    let requests = loader.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].headers.get("cookie").as_deref(),
        Some("lax=allowed"),
        "cross-site top-level GET may send Lax but must not send Strict"
    );
}

#[test]
fn post_302_rewrites_to_get_before_lax_cookie_evaluation() {
    let loader = Arc::new(ScriptedLoader::default());
    let (network, jar) = navigation(loader.clone());
    assert!(jar.borrow_mut().store_set_cookie(
        "strict=blocked; Path=/; SameSite=Strict",
        &url("http://other.test/"),
        0,
    ));
    assert!(jar.borrow_mut().store_set_cookie(
        "lax=after-get; Path=/; SameSite=Lax",
        &url("http://other.test/"),
        0,
    ));

    let request = FetchRequest::new(
        url("http://example.test/post-start"),
        Method::Post,
        browser_engine::net::HeaderMap::new(),
        Some(b"payload".to_vec()),
    );
    network
        .fetch(
            &request,
            SameSiteRequestContext::new(true, true, Method::Post),
        )
        .expect("redirect chain succeeds");

    let requests = loader.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, Method::Post);
    assert_eq!(requests[1].method, Method::Get);
    assert!(requests[1].body.is_none());
    assert_eq!(
        requests[1].headers.get("cookie").as_deref(),
        Some("lax=after-get"),
        "SameSite Lax must be evaluated using the rewritten safe method"
    );
}
