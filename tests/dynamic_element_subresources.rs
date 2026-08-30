use std::sync::{Arc, Mutex};

use browser_engine::css::parser::{Color, Value};
use browser_engine::net::{
    FetchError, FetchRequest, FetchResponse, LoadError, Resource, ResourceLoader, Url,
};
use browser_engine::script::dom_api;
use browser_engine::{Browser, PointerState};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

struct DynamicLoader {
    html: String,
    allow_cors: bool,
    requests: Arc<Mutex<Vec<FetchRequest>>>,
}

impl DynamicLoader {
    fn new(html: impl Into<String>, allow_cors: bool) -> (Self, Arc<Mutex<Vec<FetchRequest>>>) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                html: html.into(),
                allow_cors,
                requests: requests.clone(),
            },
            requests,
        )
    }

    fn response_for(&self, request: &FetchRequest) -> FetchResponse {
        let (mime, body): (&str, Vec<u8>) = match request.url.path() {
            "/late.js" => (
                "application/javascript",
                br#"document.getElementById("target").setAttribute("data-dynamic-script", "loaded");"#
                    .to_vec(),
            ),
            "/chain-a.js" => (
                "application/javascript",
                br#"const next = document.createElement("script");
                    next.setAttribute("src", "https://cdn.test/chain-b.js");
                    next.setAttribute("crossorigin", "anonymous");
                    document.getElementById("host").appendChild(next);"#
                    .to_vec(),
            ),
            "/chain-b.js" => (
                "application/javascript",
                br#"document.getElementById("target").setAttribute("data-chain", "done");"#
                    .to_vec(),
            ),
            "/count.js" => (
                "application/javascript",
                br#"const marker = document.createElement("span");
                    marker.setAttribute("class", "run");
                    document.getElementById("host").appendChild(marker);"#
                    .to_vec(),
            ),
            "/late.css" => (
                "text/css",
                b"#target { color: rgb(7, 8, 9); }".to_vec(),
            ),
            _ => ("text/plain", b"ok".to_vec()),
        };
        let mut response = FetchResponse::synthetic(request.url.clone(), 200, Some(mime), body);
        if self.allow_cors {
            response.headers.insert_raw("access-control-allow-origin", "*");
        }
        response
    }
}

impl ResourceLoader for DynamicLoader {
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

fn target_color(browser: &Browser) -> Option<Value> {
    let styled = browser
        .document()
        .style_tree(800.0, &PointerState::default());
    fn find(node: &browser_engine::style::StyledNode<'_>) -> Option<Value> {
        if node.node.as_element().and_then(|element| element.get_attr("id")) == Some("target") {
            return node.value("color").cloned();
        }
        node.children.iter().find_map(find)
    }
    find(&styled)
}

#[test]
fn parser_script_inserted_external_script_runs_before_first_paint_and_only_once() {
    let html = r#"
        <button id="noop">noop</button><div id="target"></div><div id="host"></div>
        <script>
          const late = document.createElement("script");
          late.setAttribute("src", "https://cdn.test/late.js");
          late.setAttribute("crossorigin", "anonymous");
          late.setAttribute("referrerpolicy", "no-referrer");
          document.getElementById("host").appendChild(late);
          document.getElementById("noop").addEventListener("click", () => {
            document.getElementById("target").setAttribute("data-noop", "yes");
          });
        </script>
    "#;
    let (loader, requests) = DynamicLoader::new(html, true);
    let mut browser = Browser::open(Box::new(loader), &url("https://page.test/index.html"))
        .expect("document loads");

    let target_path = dom_api::get_element_by_id(&browser.document().dom, "target").unwrap();
    let target = dom_api::node_at(&browser.document().dom, &target_path)
        .unwrap()
        .as_element()
        .unwrap();
    assert_eq!(target.get_attr("data-dynamic-script"), Some("loaded"));

    let first_requests = requests.lock().unwrap().clone();
    let late = first_requests
        .iter()
        .find(|request| request.url.path() == "/late.js")
        .expect("dynamic script request");
    assert_eq!(late.headers.get("origin").as_deref(), Some("https://page.test"));
    assert!(!late.headers.has("referer"), "dynamic referrerpolicy must reach transport");
    drop(first_requests);

    let noop = dom_api::get_element_by_id(&browser.document().dom, "noop").unwrap();
    browser.click_node(&noop);
    let requests = requests.lock().unwrap();
    assert_eq!(
        requests.iter().filter(|request| request.url.path() == "/late.js").count(),
        1,
        "the same script element must not execute or fetch twice"
    );
}

#[test]
fn click_inserted_stylesheet_uses_policy_and_updates_the_cascade() {
    let html = r#"
        <style>#target { color: rgb(1, 2, 3); }</style>
        <button id="add">add</button><p id="target">x</p><div id="host"></div>
        <script>
          document.getElementById("add").addEventListener("click", () => {
            const link = document.createElement("link");
            link.setAttribute("rel", "stylesheet");
            link.setAttribute("href", "https://cdn.test/late.css");
            link.setAttribute("crossorigin", "anonymous");
            link.setAttribute("referrerpolicy", "no-referrer");
            document.getElementById("host").appendChild(link);
          });
        </script>
    "#;
    let (loader, requests) = DynamicLoader::new(html, true);
    let mut browser = Browser::open(Box::new(loader), &url("https://page.test/index.html"))
        .expect("document loads");
    assert_eq!(target_color(&browser), Some(Value::Color(Color::rgb(1, 2, 3))));

    let add = dom_api::get_element_by_id(&browser.document().dom, "add").unwrap();
    browser.click_node(&add);
    assert_eq!(target_color(&browser), Some(Value::Color(Color::rgb(7, 8, 9))));

    let requests = requests.lock().unwrap();
    let stylesheet = requests
        .iter()
        .find(|request| request.url.path() == "/late.css")
        .expect("dynamic stylesheet request");
    assert_eq!(
        stylesheet.headers.get("origin").as_deref(),
        Some("https://page.test")
    );
    assert!(!stylesheet.headers.has("referer"));
}

#[test]
fn dynamically_loaded_script_can_insert_another_external_script_in_the_same_refresh() {
    let html = r#"
        <div id="target"></div><div id="host"></div>
        <script>
          const first = document.createElement("script");
          first.setAttribute("src", "https://cdn.test/chain-a.js");
          first.setAttribute("crossorigin", "anonymous");
          document.getElementById("host").appendChild(first);
        </script>
    "#;
    let (loader, requests) = DynamicLoader::new(html, true);
    let browser = Browser::open(Box::new(loader), &url("https://page.test/index.html"))
        .expect("document loads");

    let target_path = dom_api::get_element_by_id(&browser.document().dom, "target").unwrap();
    let target = dom_api::node_at(&browser.document().dom, &target_path)
        .unwrap()
        .as_element()
        .unwrap();
    assert_eq!(target.get_attr("data-chain"), Some("done"));

    let paths: Vec<String> = requests
        .lock()
        .unwrap()
        .iter()
        .map(|request| request.url.path().to_string())
        .collect();
    assert_eq!(paths, vec!["/chain-a.js", "/chain-b.js"]);
}

#[test]
fn distinct_dynamic_script_elements_with_the_same_url_each_execute_once() {
    let html = r#"
        <button id="add">add</button><div id="host"></div>
        <script>
          document.getElementById("add").addEventListener("click", () => {
            const one = document.createElement("script");
            one.setAttribute("src", "https://cdn.test/count.js");
            one.setAttribute("crossorigin", "anonymous");
            document.getElementById("host").appendChild(one);
            const two = document.createElement("script");
            two.setAttribute("src", "https://cdn.test/count.js");
            two.setAttribute("crossorigin", "anonymous");
            document.getElementById("host").appendChild(two);
          });
        </script>
    "#;
    let (loader, requests) = DynamicLoader::new(html, true);
    let mut browser = Browser::open(Box::new(loader), &url("https://page.test/index.html"))
        .expect("document loads");
    let add = dom_api::get_element_by_id(&browser.document().dom, "add").unwrap();
    browser.click_node(&add);

    assert_eq!(
        dom_api::query_selector_all(&browser.document().dom, &[], ".run").len(),
        2,
        "resource identity is the element, not the URL"
    );
    assert_eq!(
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.url.path() == "/count.js")
            .count(),
        2
    );
}

#[test]
fn cors_failure_blocks_dynamic_script_and_stylesheet_without_retrying_raw_loader() {
    let html = r#"
        <style>#target { color: rgb(1, 2, 3); }</style>
        <button id="add">add</button><p id="target">x</p><div id="host"></div>
        <script>
          document.getElementById("add").addEventListener("click", () => {
            const script = document.createElement("script");
            script.setAttribute("src", "https://cdn.test/late.js");
            script.setAttribute("crossorigin", "anonymous");
            document.getElementById("host").appendChild(script);
            const link = document.createElement("link");
            link.setAttribute("rel", "stylesheet");
            link.setAttribute("href", "https://cdn.test/late.css");
            link.setAttribute("crossorigin", "anonymous");
            document.getElementById("host").appendChild(link);
          });
        </script>
    "#;
    let (loader, requests) = DynamicLoader::new(html, false);
    let mut browser = Browser::open(Box::new(loader), &url("https://page.test/index.html"))
        .expect("document survives broken dynamic subresources");
    let add = dom_api::get_element_by_id(&browser.document().dom, "add").unwrap();
    browser.click_node(&add);

    let target_path = dom_api::get_element_by_id(&browser.document().dom, "target").unwrap();
    let target = dom_api::node_at(&browser.document().dom, &target_path)
        .unwrap()
        .as_element()
        .unwrap();
    assert_eq!(target.get_attr("data-dynamic-script"), None);
    assert_eq!(target_color(&browser), Some(Value::Color(Color::rgb(1, 2, 3))));
    assert!(
        browser
            .document()
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("CORS"))
            .count()
            >= 2
    );

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|request| {
        request.headers.get("origin").as_deref() == Some("https://page.test")
    }));
}
