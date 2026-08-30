use std::sync::{Arc, Mutex};

use browser_engine::browser::ClickOutcome;
use browser_engine::net::{
    FetchError, FetchRequest, FetchResponse, LoadError, Method, Resource, ResourceLoader, Url,
};
use browser_engine::script::dom_api;
use browser_engine::Browser;

#[derive(Clone, Default)]
struct RecordingLoader {
    requests: Arc<Mutex<Vec<FetchRequest>>>,
}

impl RecordingLoader {
    fn requests(&self) -> Vec<FetchRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl ResourceLoader for RecordingLoader {
    fn load(&self, url: &Url) -> Result<Resource, LoadError> {
        let html = if url.path() == "/form" {
            r#"<title>Form</title><form method="post" action="https://b.test/submit"><input name="q" value="v"><button id="go">Go</button></form>"#
        } else {
            "<title>Start</title><p>start</p>"
        };
        Ok(Resource::new(
            url.clone(),
            Some("text/html".into()),
            html.as_bytes().to_vec(),
        ))
    }

    fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        self.requests.lock().unwrap().push(request.clone());

        let mut final_url = request.url.clone();
        if request.url.path() == "/learn" {
            final_url = Url::parse("https://example.test/learn").unwrap();
        } else if request.url.path() == "/submit" {
            final_url = if request.url.host() == "example.test" {
                Url::parse("https://example.test/saved").unwrap()
            } else {
                Url::parse("https://b.test/saved").unwrap()
            };
        }

        let body = if request.url.path() == "/hsts-form" {
            br#"<title>HSTS Form</title><form method="post" action="http://example.test/submit"><input name="q" value="v"><button id="go">Go</button></form>"#.to_vec()
        } else {
            b"<title>Loaded</title><p>ok</p>".to_vec()
        };
        let mut response = FetchResponse::synthetic(final_url, 200, Some("text/html"), body);
        if request.url.path() == "/next" {
            response
                .headers
                .append_raw("set-cookie", "seen=1; Path=/; SameSite=Lax");
        }
        if request.url.path() == "/learn" {
            response
                .headers
                .append_raw("strict-transport-security", "max-age=60");
            response
                .headers
                .append_raw("set-cookie", "secure_lax=1; Path=/; Secure; SameSite=Lax");
            response.headers.append_raw(
                "set-cookie",
                "secure_strict=1; Path=/; Secure; SameSite=Strict",
            );
        }
        Ok(response)
    }
}

fn url(input: &str) -> Url {
    Url::parse(input).unwrap()
}

fn seed_cookie(browser: &Browser, source: &str, value: &str) {
    assert!(browser
        .cookie_jar()
        .borrow_mut()
        .store_set_cookie(value, &url(source), 0));
}

#[test]
fn same_site_get_navigation_sends_strict_cookie_and_absorbs_response_cookie() {
    let loader = RecordingLoader::default();
    let probe = loader.clone();
    let start = url("http://example.test/start");
    let mut browser = Browser::open(Box::new(loader), &start).unwrap();

    seed_cookie(
        &browser,
        "http://example.test/",
        "strict=s; Path=/; SameSite=Strict",
    );

    browser.navigate(&url("http://example.test/next")).unwrap();

    let requests = probe.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, Method::Get);
    assert_eq!(
        requests[0].headers.get("cookie").as_deref(),
        Some("strict=s")
    );
    assert_eq!(browser.document().title().as_deref(), Some("Loaded"));

    let visible = browser
        .cookie_jar()
        .borrow()
        .get_document_cookie(browser.url(), 0);
    assert!(visible.contains("strict=s"), "{visible}");
    assert!(visible.contains("seen=1"), "{visible}");
}

#[test]
fn cross_site_get_navigation_sends_lax_and_none_but_not_strict() {
    let loader = RecordingLoader::default();
    let probe = loader.clone();
    let mut browser = Browser::open(Box::new(loader), &url("https://a.test/start")).unwrap();

    for cookie in [
        "strict=s; Path=/; Secure; SameSite=Strict",
        "lax=l; Path=/; Secure; SameSite=Lax",
        "none=n; Path=/; Secure; SameSite=None",
    ] {
        seed_cookie(&browser, "https://b.test/", cookie);
    }

    browser.navigate(&url("https://b.test/next")).unwrap();

    let requests = probe.requests();
    let cookie = requests[0].headers.get("cookie").unwrap();
    assert!(cookie.contains("lax=l"), "{cookie}");
    assert!(cookie.contains("none=n"), "{cookie}");
    assert!(!cookie.contains("strict=s"), "{cookie}");
}

#[test]
fn cross_site_post_navigation_sends_only_samesite_none() {
    let loader = RecordingLoader::default();
    let probe = loader.clone();
    let mut browser = Browser::open(Box::new(loader), &url("https://a.test/form")).unwrap();

    for cookie in [
        "strict=s; Path=/; Secure; SameSite=Strict",
        "lax=l; Path=/; Secure; SameSite=Lax",
        "none=n; Path=/; Secure; SameSite=None",
    ] {
        seed_cookie(&browser, "https://b.test/", cookie);
    }

    let button = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();
    let outcome = browser.click_node(&button);
    assert!(matches!(outcome, ClickOutcome::Navigated(_)), "{outcome:?}");

    let requests = probe.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, Method::Post);
    assert_eq!(requests[0].url.to_string(), "https://b.test/submit");
    assert_eq!(requests[0].headers.get("cookie").as_deref(), Some("none=n"));
    assert_eq!(browser.url().to_string(), "https://b.test/saved");
}

#[test]
fn hsts_learned_by_navigation_upgrades_the_next_browser_navigation() {
    let loader = RecordingLoader::default();
    let probe = loader.clone();
    let mut browser = Browser::open(Box::new(loader), &url("http://example.test/start")).unwrap();

    browser.navigate(&url("http://example.test/learn")).unwrap();
    assert_eq!(browser.url().to_string(), "https://example.test/learn");

    browser
        .navigate(&url("http://example.test/account"))
        .unwrap();

    let requests = probe.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].url.to_string(), "http://example.test/learn");
    assert_eq!(requests[1].url.to_string(), "https://example.test/account");
    let cookie = requests[1].headers.get("cookie").unwrap();
    assert!(cookie.contains("secure_lax=1"), "{cookie}");
    assert!(cookie.contains("secure_strict=1"), "{cookie}");
}

#[test]
fn hsts_upgraded_same_site_post_keeps_strict_and_lax_cookies() {
    let loader = RecordingLoader::default();
    let probe = loader.clone();
    let mut browser = Browser::open(Box::new(loader), &url("http://example.test/start")).unwrap();

    browser.navigate(&url("http://example.test/learn")).unwrap();
    browser
        .navigate(&url("http://example.test/hsts-form"))
        .unwrap();
    assert_eq!(browser.url().to_string(), "https://example.test/hsts-form");

    let button = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();
    let outcome = browser.click_node(&button);
    assert!(matches!(outcome, ClickOutcome::Navigated(_)), "{outcome:?}");

    let requests = probe.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[2].method, Method::Post);
    assert_eq!(requests[2].url.to_string(), "https://example.test/submit");
    let cookie = requests[2].headers.get("cookie").unwrap();
    assert!(cookie.contains("secure_strict=1"), "{cookie}");
    assert!(cookie.contains("secure_lax=1"), "{cookie}");
    assert_eq!(browser.url().to_string(), "https://example.test/saved");
}
