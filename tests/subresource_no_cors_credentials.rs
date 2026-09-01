use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use browser_engine::cookie::CookieJar;
use browser_engine::eventloop::ManualClock;
use browser_engine::hsts::HstsCache;
use browser_engine::net::{
    FetchError, FetchRequest, FetchResponse, LoadError, Resource, ResourceLoader, Url,
};
use browser_engine::script::dom_api;
use browser_engine::NavigationNetwork;

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

struct Loader {
    requests: Arc<Mutex<Vec<FetchRequest>>>,
}

impl ResourceLoader for Loader {
    fn load(&self, target: &Url) -> Result<Resource, LoadError> {
        Err(LoadError::NotFound(target.to_string()))
    }

    fn fetch_once(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        self.requests.lock().unwrap().push(request.clone());
        Ok(FetchResponse::synthetic(
            request.url.clone(),
            200,
            Some("application/javascript"),
            br#"document.getElementById("target").setAttribute("data-script", "loaded");"#
                .to_vec(),
        ))
    }
}

#[test]
fn missing_crossorigin_keeps_no_cors_include_credentials_behavior() {
    let html = r#"<p id="target">x</p>
        <script src="https://cdn.test/app.js"></script>"#;
    let requests = Arc::new(Mutex::new(Vec::new()));
    let loader: Arc<dyn ResourceLoader> = Arc::new(Loader {
        requests: requests.clone(),
    });
    let clock = Rc::new(ManualClock::new());
    let jar = Rc::new(RefCell::new(CookieJar::with_clock(clock.clone())));
    jar.borrow_mut().store_set_cookie(
        "cdn=default; Path=/; SameSite=None; Secure",
        &url("https://cdn.test/"),
        0,
    );
    let hsts = Rc::new(RefCell::new(HstsCache::new()));
    let navigation = NavigationNetwork::new(loader, jar.clone(), hsts, clock);
    let response = FetchResponse::synthetic(
        url("https://page.test/index.html"),
        200,
        Some("text/html"),
        html.as_bytes().to_vec(),
    );

    let document = browser_engine::document::Document::from_response_with_session_subresources(
        &response,
        &navigation,
        None,
        Some(jar),
    );

    let target_path = dom_api::get_element_by_id(&document.dom, "target").unwrap();
    let target = dom_api::node_at(&document.dom, &target_path)
        .unwrap()
        .as_element()
        .unwrap();
    assert_eq!(target.get_attr("data-script"), Some("loaded"));

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].headers.get("cookie").as_deref(),
        Some("cdn=default"),
        "an element request without crossorigin is no-cors with credentials mode include"
    );
    assert!(
        !requests[0].headers.has("origin"),
        "the absent crossorigin attribute must not turn the request into CORS mode"
    );
}
