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

#[derive(Clone, Copy)]
enum ReplyMode {
    Wildcard,
    ExactWithCredentials,
    SameOrigin,
    RedirectToWildcardCdn,
}

struct RecordingCorsLoader {
    seen: Arc<Mutex<Vec<FetchRequest>>>,
    mode: ReplyMode,
    set_cookie: Option<&'static str>,
}

impl ResourceLoader for RecordingCorsLoader {
    fn load(&self, target: &Url) -> Result<Resource, LoadError> {
        Err(LoadError::NotFound(target.to_string()))
    }

    fn fetch_once(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        self.seen.lock().unwrap().push(request.clone());

        if matches!(self.mode, ReplyMode::RedirectToWildcardCdn)
            && request.url.host().eq_ignore_ascii_case("page.test")
        {
            let mut response =
                FetchResponse::synthetic(request.url.clone(), 302, Some("text/plain"), Vec::new());
            response
                .headers
                .insert_raw("location", "https://cdn.test/final.js");
            return Ok(response);
        }

        let mut response = FetchResponse::synthetic(
            request.url.clone(),
            200,
            Some("application/javascript"),
            b"ok".to_vec(),
        );

        match self.mode {
            ReplyMode::Wildcard | ReplyMode::RedirectToWildcardCdn => {
                response
                    .headers
                    .insert_raw("access-control-allow-origin", "*");
            }
            ReplyMode::ExactWithCredentials => {
                response
                    .headers
                    .insert_raw("access-control-allow-origin", "https://page.test");
                response
                    .headers
                    .insert_raw("access-control-allow-credentials", "true");
            }
            ReplyMode::SameOrigin => {}
        }

        if let Some(value) = self.set_cookie {
            response.headers.append_raw("set-cookie", value);
        }

        Ok(response)
    }
}

fn network(
    mode: ReplyMode,
    set_cookie: Option<&'static str>,
) -> (NavigationNetwork, Arc<Mutex<Vec<FetchRequest>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let clock = Rc::new(ManualClock::new());
    let jar = Rc::new(std::cell::RefCell::new(CookieJar::with_clock(
        clock.clone(),
    )));
    let hsts = Rc::new(std::cell::RefCell::new(HstsCache::new()));
    let loader = RecordingCorsLoader {
        seen: seen.clone(),
        mode,
        set_cookie,
    };

    (
        NavigationNetwork::new(Arc::new(loader), jar, hsts, clock),
        seen,
    )
}

fn document() -> DocumentReferrerContext {
    DocumentReferrerContext::from_url(url("https://page.test/index.html"))
}

fn context(same_site: bool) -> SameSiteRequestContext {
    SameSiteRequestContext::new(same_site, false, Method::Get)
}

fn seed_cookie(network: &NavigationNetwork, source: &str, value: &str) {
    assert!(
        network
            .cookie_jar()
            .borrow_mut()
            .store_set_cookie(value, &url(source), 0),
        "cookie should be accepted"
    );
}

#[test]
fn anonymous_cross_origin_replaces_origin_and_omits_cookies() {
    let (network, seen) = network(ReplyMode::Wildcard, None);
    seed_cookie(
        &network,
        "https://cdn.test/",
        "cdn_session=secret; Path=/; SameSite=None; Secure",
    );

    let mut request = FetchRequest::get(url("https://cdn.test/app.js"));
    request
        .headers
        .insert_raw("origin", "https://attacker.test");
    request.headers.insert_raw("cookie", "forged=1");

    document()
        .fetch_subresource_with_cors_credentials(
            &network,
            &request,
            context(false),
            None,
            Some("anonymous"),
        )
        .expect("anonymous wildcard CORS should succeed");

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].headers.get("origin").as_deref(),
        Some("https://page.test")
    );
    assert_eq!(seen[0].headers.get("cookie"), None);
}

#[test]
fn use_credentials_cross_origin_includes_eligible_cookie() {
    let (network, seen) = network(ReplyMode::ExactWithCredentials, None);
    seed_cookie(
        &network,
        "https://cdn.test/",
        "cdn_session=secret; Path=/; SameSite=None; Secure",
    );

    document()
        .fetch_subresource_with_cors_credentials(
            &network,
            &FetchRequest::get(url("https://cdn.test/app.js")),
            context(false),
            None,
            Some("use-credentials"),
        )
        .expect("credentialed CORS should succeed with exact permission");

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].headers.get("origin").as_deref(),
        Some("https://page.test")
    );
    assert_eq!(
        seen[0].headers.get("cookie").as_deref(),
        Some("cdn_session=secret")
    );
}

#[test]
fn anonymous_same_origin_keeps_same_origin_credentials() {
    let (network, seen) = network(ReplyMode::SameOrigin, None);
    seed_cookie(
        &network,
        "https://page.test/",
        "page_session=secret; Path=/; SameSite=None; Secure",
    );

    document()
        .fetch_subresource_with_cors_credentials(
            &network,
            &FetchRequest::get(url("https://page.test/app.js")),
            context(true),
            None,
            Some("anonymous"),
        )
        .expect("same-origin anonymous CORS should retain same-origin credentials");

    let seen = seen.lock().unwrap();
    assert_eq!(
        seen[0].headers.get("cookie").as_deref(),
        Some("page_session=secret")
    );
}

#[test]
fn anonymous_cross_origin_does_not_store_set_cookie() {
    let (network, _seen) = network(
        ReplyMode::Wildcard,
        Some("fresh=1; Path=/; SameSite=None; Secure"),
    );

    let response = document()
        .fetch_subresource_with_cors_credentials(
            &network,
            &FetchRequest::get(url("https://cdn.test/app.js")),
            context(false),
            None,
            Some("anonymous"),
        )
        .expect("anonymous wildcard CORS should succeed");

    assert_eq!(response.headers.get("set-cookie"), None);
    assert_eq!(
        network
            .cookie_jar()
            .borrow()
            .get_http_cookie_header(&url("https://cdn.test/"), 0),
        None
    );
}

#[test]
fn use_credentials_cross_origin_stores_set_cookie_but_hides_header() {
    let (network, _seen) = network(
        ReplyMode::ExactWithCredentials,
        Some("fresh=1; Path=/; SameSite=None; Secure"),
    );

    let response = document()
        .fetch_subresource_with_cors_credentials(
            &network,
            &FetchRequest::get(url("https://cdn.test/app.js")),
            context(false),
            None,
            Some("use-credentials"),
        )
        .expect("credentialed CORS should accept and store response cookie");

    assert_eq!(response.headers.get("set-cookie"), None);
    assert_eq!(
        network
            .cookie_jar()
            .borrow()
            .get_http_cookie_header(&url("https://cdn.test/"), 0)
            .as_deref(),
        Some("fresh=1")
    );
}

#[test]
fn anonymous_redirect_recomputes_cookie_eligibility_per_hop() {
    let (network, seen) = network(ReplyMode::RedirectToWildcardCdn, None);
    seed_cookie(
        &network,
        "https://page.test/",
        "page_session=first; Path=/; SameSite=None; Secure",
    );
    seed_cookie(
        &network,
        "https://cdn.test/",
        "cdn_session=second; Path=/; SameSite=None; Secure",
    );

    let response = document()
        .fetch_subresource_with_cors_credentials(
            &network,
            &FetchRequest::get(url("https://page.test/start.js")),
            context(true),
            None,
            Some("anonymous"),
        )
        .expect("same-origin to cross-origin redirect should pass wildcard CORS");

    assert!(response.redirected);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(
        seen[0].headers.get("cookie").as_deref(),
        Some("page_session=first")
    );
    assert_eq!(seen[1].headers.get("cookie"), None);
    assert_eq!(
        seen[1].headers.get("origin").as_deref(),
        Some("https://page.test")
    );
}
