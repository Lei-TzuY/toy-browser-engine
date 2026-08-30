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
use browser_engine::{Browser, DocumentReferrerContext, NavigationNetwork};

const PPM: &[u8] = b"P6\n1 1\n255\n\x10\x20\x30";

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

#[derive(Clone, Copy)]
enum CorsReply {
    None,
    Wildcard,
    Credentialed,
}

struct ElementLoader {
    html: String,
    cors: CorsReply,
    requests: Arc<Mutex<Vec<FetchRequest>>>,
}

impl ElementLoader {
    fn new(html: impl Into<String>, cors: CorsReply) -> (Self, Arc<Mutex<Vec<FetchRequest>>>) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                html: html.into(),
                cors,
                requests: requests.clone(),
            },
            requests,
        )
    }

    fn response_for(&self, request: &FetchRequest) -> FetchResponse {
        let path = request.url.path();
        let (mime, body) = if path.ends_with("app.js") {
            (
                "application/javascript",
                br#"document.getElementById("target").setAttribute("data-script", "loaded");"#
                    .to_vec(),
            )
        } else if path.ends_with("style.css") {
            ("text/css", b"#target { color: rgb(7, 8, 9); }".to_vec())
        } else if path.ends_with("dot.ppm") {
            ("image/x-portable-pixmap", PPM.to_vec())
        } else {
            ("text/plain", b"ok".to_vec())
        };
        let mut response = FetchResponse::synthetic(request.url.clone(), 200, Some(mime), body);
        match self.cors {
            CorsReply::None => {}
            CorsReply::Wildcard => {
                response
                    .headers
                    .insert_raw("access-control-allow-origin", "*");
            }
            CorsReply::Credentialed => {
                response
                    .headers
                    .insert_raw("access-control-allow-origin", "https://page.test");
                response
                    .headers
                    .insert_raw("access-control-allow-credentials", "true");
            }
        }
        response
    }
}

impl ResourceLoader for ElementLoader {
    fn load(&self, target: &Url) -> Result<Resource, LoadError> {
        Err(LoadError::NotFound(target.to_string()))
    }

    fn load_response_once(
        &self,
        request: &FetchRequest,
    ) -> Result<Option<FetchResponse>, LoadError> {
        if request.url.to_string() == "https://page.test/index.html" {
            Ok(Some(FetchResponse::synthetic(
                request.url.clone(),
                200,
                Some("text/html"),
                self.html.as_bytes().to_vec(),
            )))
        } else {
            Ok(None)
        }
    }

    fn fetch_once(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        self.requests.lock().unwrap().push(request.clone());
        Ok(self.response_for(request))
    }
}

fn bootstrap_html(crossorigin: &str) -> String {
    format!(
        r#"<html><head>
            <link rel="stylesheet" href="https://cdn.test/style.css" crossorigin="{crossorigin}">
          </head><body>
            <p id="target">x</p>
            <script src="https://cdn.test/app.js" crossorigin="{crossorigin}" referrerpolicy="no-referrer"></script>
            <img src="https://cdn.test/dot.ppm" crossorigin="{crossorigin}">
          </body></html>"#
    )
}

#[test]
fn browser_bootstrap_routes_script_style_and_image_through_cors_policy() {
    let (loader, requests) = ElementLoader::new(bootstrap_html("anonymous"), CorsReply::Wildcard);
    let browser = Browser::open(Box::new(loader), &url("https://page.test/index.html"))
        .expect("document loads");

    let target = dom_api::get_element_by_id(&browser.document().dom, "target").unwrap();
    let target = dom_api::node_at(&browser.document().dom, &target)
        .unwrap()
        .as_element()
        .unwrap();
    assert_eq!(target.get_attr("data-script"), Some("loaded"));

    let image = browser
        .document()
        .images
        .get(&url("https://cdn.test/dot.ppm"));
    assert!(image.is_some(), "CORS-approved image should decode");
    assert!(
        browser.document().diagnostics.is_empty(),
        "{:?}",
        browser.document().diagnostics
    );

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    for request in requests.iter() {
        assert_eq!(
            request.headers.get("origin").as_deref(),
            Some("https://page.test")
        );
        assert!(!request.headers.has("cookie"));
    }
    let script = requests
        .iter()
        .find(|request| request.url.path().ends_with("app.js"))
        .unwrap();
    assert!(
        !script.headers.has("referer"),
        "element referrerpolicy must reach transport"
    );
}

#[test]
fn browser_bootstrap_blocks_cors_elements_without_response_permission() {
    let (loader, _) = ElementLoader::new(bootstrap_html("anonymous"), CorsReply::None);
    let browser = Browser::open(Box::new(loader), &url("https://page.test/index.html"))
        .expect("the document survives broken subresources");

    let target_path = dom_api::get_element_by_id(&browser.document().dom, "target").unwrap();
    let target = dom_api::node_at(&browser.document().dom, &target_path)
        .unwrap()
        .as_element()
        .unwrap();
    assert_eq!(target.get_attr("data-script"), None);
    assert!(browser
        .document()
        .images
        .get(&url("https://cdn.test/dot.ppm"))
        .is_none());
    assert_eq!(browser.document().diagnostics.len(), 3);
    assert!(browser
        .document()
        .diagnostics
        .iter()
        .all(|diagnostic| diagnostic.message.contains("CORS")));
}

#[test]
fn use_credentials_element_reaches_browser_cookie_selection() {
    let html = r#"<p id="target">x</p>
        <script src="https://cdn.test/app.js" crossorigin="use-credentials"></script>"#;
    let (loader, requests) = ElementLoader::new(html, CorsReply::Credentialed);
    let loader: Arc<dyn ResourceLoader> = Arc::new(loader);
    let clock = Rc::new(ManualClock::new());
    let jar = Rc::new(RefCell::new(CookieJar::with_clock(clock.clone())));
    jar.borrow_mut().store_set_cookie(
        "cdn=credential; Path=/; SameSite=None; Secure",
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
        Some("cdn=credential")
    );
    assert_eq!(
        requests[0].headers.get("origin").as_deref(),
        Some("https://page.test")
    );
}

#[test]
fn dynamic_crossorigin_image_refresh_does_not_fall_back_to_raw_loader() {
    let html = r#"<button id="add">add</button><div id="host"></div>
        <script>
          document.getElementById("add").addEventListener("click", () => {
            const img = document.createElement("img");
            img.setAttribute("src", "https://cdn.test/dot.ppm");
            img.setAttribute("crossorigin", "anonymous");
            document.getElementById("host").appendChild(img);
          });
        </script>"#;
    let (loader, requests) = ElementLoader::new(html, CorsReply::None);
    let mut browser = Browser::open(Box::new(loader), &url("https://page.test/index.html"))
        .expect("document loads");
    let button = dom_api::get_element_by_id(&browser.document().dom, "add").unwrap();
    browser.click_node(&button);

    assert!(
        browser
            .document()
            .images
            .get(&url("https://cdn.test/dot.ppm"))
            .is_none(),
        "missing ACAO must keep dynamically-added image blocked"
    );
    assert!(browser
        .document()
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.message.contains("CORS")));

    let requests = requests.lock().unwrap();
    let image_request = requests
        .iter()
        .find(|request| request.url.path().ends_with("dot.ppm"))
        .expect("dynamic image reached policy-aware transport");
    assert_eq!(
        image_request.headers.get("origin").as_deref(),
        Some("https://page.test")
    );
}
