use std::sync::{Arc, Mutex};

use browser_engine::net::{
    FetchError, FetchRequest, FetchResponse, LoadError, Resource, ResourceLoader, Url,
};
use browser_engine::script::dom_api;
use browser_engine::Browser;

const PPM: &[u8] = b"P6\n1 1\n255\n\x10\x20\x30";

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

struct ReferrerLoader {
    html: String,
    response_policy: &'static str,
    requests: Arc<Mutex<Vec<FetchRequest>>>,
}

impl ReferrerLoader {
    fn new(
        html: impl Into<String>,
        response_policy: &'static str,
    ) -> (Self, Arc<Mutex<Vec<FetchRequest>>>) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                html: html.into(),
                response_policy,
                requests: requests.clone(),
            },
            requests,
        )
    }
}

impl ResourceLoader for ReferrerLoader {
    fn load(&self, target: &Url) -> Result<Resource, LoadError> {
        Err(LoadError::NotFound(target.to_string()))
    }

    fn load_response_once(
        &self,
        request: &FetchRequest,
    ) -> Result<Option<FetchResponse>, LoadError> {
        if request.url.to_string() != "https://page.test/index.html" {
            return Ok(None);
        }
        let mut response = FetchResponse::synthetic(
            request.url.clone(),
            200,
            Some("text/html"),
            self.html.as_bytes().to_vec(),
        );
        response
            .headers
            .insert_raw("referrer-policy", self.response_policy);
        Ok(Some(response))
    }

    fn fetch_once(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        self.requests.lock().unwrap().push(request.clone());
        let mut response = FetchResponse::synthetic(
            request.url.clone(),
            200,
            Some("image/x-portable-pixmap"),
            PPM.to_vec(),
        );
        response
            .headers
            .insert_raw("access-control-allow-origin", "*");
        Ok(response)
    }
}

fn dynamic_image_page(extra_head: &str) -> String {
    format!(
        r#"<html><head>{extra_head}</head><body>
        <button id="add">add</button><div id="host"></div>
        <script>
          document.getElementById("add").addEventListener("click", () => {{
            const img = document.createElement("img");
            img.setAttribute("src", "https://cdn.test/dot.ppm");
            img.setAttribute("crossorigin", "anonymous");
            document.getElementById("host").appendChild(img);
          }});
        </script>
        </body></html>"#
    )
}

#[test]
fn response_header_policy_survives_until_dynamic_image_fetch() {
    let (loader, requests) = ReferrerLoader::new(dynamic_image_page(""), "unsafe-url");
    let mut browser = Browser::open(Box::new(loader), &url("https://page.test/index.html"))
        .expect("document loads");

    let button = dom_api::get_element_by_id(&browser.document().dom, "add").unwrap();
    browser.click_node(&button);

    let requests = requests.lock().unwrap();
    let image = requests
        .iter()
        .find(|request| request.url.path().ends_with("dot.ppm"))
        .expect("dynamic image request");
    assert_eq!(
        image.headers.get("referer").as_deref(),
        Some("https://page.test/index.html"),
        "dynamic resource must retain the final response's unsafe-url policy"
    );
    assert_eq!(
        image.headers.get("origin").as_deref(),
        Some("https://page.test")
    );
    assert!(browser
        .document()
        .images
        .get(&url("https://cdn.test/dot.ppm"))
        .is_some());
}

#[test]
fn parser_time_meta_override_is_frozen_for_later_dynamic_fetches() {
    let (loader, requests) = ReferrerLoader::new(
        dynamic_image_page(r#"<meta id="policy" name="referrer" content="no-referrer">"#),
        "unsafe-url",
    );
    let mut browser = Browser::open(Box::new(loader), &url("https://page.test/index.html"))
        .expect("document loads");

    // Referrer metadata is processed while the document is committed. Later
    // script mutation of the live meta element must not retroactively replace
    // that already-selected policy. This specifically catches the old #159
    // behavior that reconstructed policy from the mutable DOM on every image
    // refresh.
    {
        let document = browser.document_mut();
        document.runtime.run_script(
            &mut document.dom,
            r#"document.getElementById("policy").setAttribute("content", "unsafe-url");"#,
        );
    }

    let button = dom_api::get_element_by_id(&browser.document().dom, "add").unwrap();
    browser.click_node(&button);

    let requests = requests.lock().unwrap();
    let image = requests
        .iter()
        .find(|request| request.url.path().ends_with("dot.ppm"))
        .expect("dynamic image request");
    assert!(
        !image.headers.has("referer"),
        "parser-time meta policy must remain authoritative after live DOM mutation"
    );
}
