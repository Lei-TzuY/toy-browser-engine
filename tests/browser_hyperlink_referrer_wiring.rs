use std::sync::{Arc, Mutex};

use browser_engine::browser::ClickOutcome;
use browser_engine::net::{
    FetchError, FetchRequest, FetchResponse, LoadError, Resource, ResourceLoader, Url,
};
use browser_engine::script::dom_api;
use browser_engine::Browser;

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

struct LinkLoader {
    source_html: String,
    source_policy: Option<&'static str>,
    requests: Arc<Mutex<Vec<FetchRequest>>>,
}

impl LinkLoader {
    fn new(
        source_html: impl Into<String>,
        source_policy: Option<&'static str>,
    ) -> (Self, Arc<Mutex<Vec<FetchRequest>>>) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                source_html: source_html.into(),
                source_policy,
                requests: requests.clone(),
            },
            requests,
        )
    }
}

impl ResourceLoader for LinkLoader {
    fn load(&self, target: &Url) -> Result<Resource, LoadError> {
        Err(LoadError::NotFound(target.to_string()))
    }

    fn load_response_once(
        &self,
        request: &FetchRequest,
    ) -> Result<Option<FetchResponse>, LoadError> {
        if request.url.host().eq_ignore_ascii_case("source.test") {
            let mut response = FetchResponse::synthetic(
                request.url.clone(),
                200,
                Some("text/html"),
                self.source_html.as_bytes().to_vec(),
            );
            if let Some(policy) = self.source_policy {
                response.headers.insert_raw("referrer-policy", policy);
            }
            return Ok(Some(response));
        }
        Ok(None)
    }

    fn fetch_once(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        self.requests.lock().unwrap().push(request.clone());

        if request.url.path() == "/redirect" {
            let mut response = FetchResponse::synthetic(
                request.url.clone(),
                302,
                Some("text/plain"),
                Vec::new(),
            );
            response
                .headers
                .insert_raw("location", "https://target.test/final");
            response
                .headers
                .insert_raw("referrer-policy", "no-referrer");
            return Ok(response);
        }

        let body = if request.url.path() == "/landing" {
            br#"<a id="next" href="https://third.test/final">next</a>"#.to_vec()
        } else {
            b"<p>ok</p>".to_vec()
        };
        Ok(FetchResponse::synthetic(
            request.url.clone(),
            200,
            Some("text/html"),
            body,
        ))
    }
}

fn click_id(browser: &mut Browser, id: &str) -> ClickOutcome {
    let path = dom_api::get_element_by_id(&browser.document().dom, id).expect("element exists");
    browser.click_node(&path)
}

#[test]
fn element_referrerpolicy_reaches_the_browser_navigation_request() {
    let (loader, requests) = LinkLoader::new(
        r#"<a id="go" href="https://target.test/final" referrerpolicy="unsafe-url">go</a>"#,
        Some("origin"),
    );
    let source = url("https://source.test/private/page?q=1#fragment");
    let mut browser = Browser::open(Box::new(loader), &source).expect("source document loads");

    assert_eq!(
        click_id(&mut browser, "go"),
        ClickOutcome::Navigated(url("https://target.test/final"))
    );

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].headers.get("referer").as_deref(),
        Some("https://source.test/private/page?q=1")
    );
}

#[test]
fn rel_noreferrer_overrides_an_explicit_unsafe_url_policy_in_browser_clicks() {
    let (loader, requests) = LinkLoader::new(
        r#"<a id="go" href="https://target.test/final" referrerpolicy="unsafe-url" rel="noopener noreferrer">go</a>"#,
        Some("unsafe-url"),
    );
    let mut browser = Browser::open(
        Box::new(loader),
        &url("https://source.test/private/page?q=1"),
    )
    .expect("source document loads");

    click_id(&mut browser, "go");

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].headers.get("referer").is_none());
}

#[test]
fn redirect_response_tightens_the_policy_for_the_next_browser_hop() {
    let (loader, requests) = LinkLoader::new(
        r#"<a id="go" href="https://target.test/redirect" referrerpolicy="unsafe-url">go</a>"#,
        Some("origin"),
    );
    let mut browser = Browser::open(
        Box::new(loader),
        &url("https://source.test/private/page?q=1"),
    )
    .expect("source document loads");

    assert_eq!(
        click_id(&mut browser, "go"),
        ClickOutcome::Navigated(url("https://target.test/redirect"))
    );
    assert_eq!(browser.url(), &url("https://target.test/final"));

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].headers.get("referer").as_deref(),
        Some("https://source.test/private/page?q=1")
    );
    assert!(
        requests[1].headers.get("referer").is_none(),
        "the redirect response's no-referrer policy must govern the second hop"
    );
}

#[test]
fn click_listener_mutation_changes_the_hyperlink_default_action_policy() {
    let (loader, requests) = LinkLoader::new(
        r#"
        <a id="go" href="https://target.test/final" referrerpolicy="unsafe-url">go</a>
        <script>
          document.getElementById("go").addEventListener("click", () => {
            document.getElementById("go").setAttribute("referrerpolicy", "no-referrer");
          });
        </script>
        "#,
        Some("unsafe-url"),
    );
    let mut browser = Browser::open(
        Box::new(loader),
        &url("https://source.test/private/page?q=1"),
    )
    .expect("source document loads");

    click_id(&mut browser, "go");

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].headers.get("referer").is_none(),
        "default action must snapshot the anchor after click listeners run"
    );
}

#[test]
fn one_link_override_does_not_become_the_next_documents_policy() {
    let (loader, requests) = LinkLoader::new(
        r#"<a id="go" href="https://target.test/landing" referrerpolicy="no-referrer">go</a>"#,
        Some("unsafe-url"),
    );
    let mut browser = Browser::open(
        Box::new(loader),
        &url("https://source.test/private/page?q=1"),
    )
    .expect("source document loads");

    click_id(&mut browser, "go");
    assert_eq!(browser.url(), &url("https://target.test/landing"));
    click_id(&mut browser, "next");

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].headers.get("referer").is_none());
    assert_eq!(
        requests[1].headers.get("referer").as_deref(),
        Some("https://target.test/"),
        "the committed target document must use its own default policy"
    );
}
