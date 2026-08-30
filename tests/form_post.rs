use std::sync::{Arc, Mutex};

use browser_engine::browser::ClickOutcome;
use browser_engine::net::{
    FetchError, FetchRequest, FetchResponse, LoadError, Method, Resource, Url,
};
use browser_engine::script::dom_api;
use browser_engine::{Browser, ResourceLoader};

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
        match url.path() {
            "/form.html" => Ok(Resource::new(
                url.clone(),
                Some("text/html".into()),
                br#"<title>Form</title>
                    <form id="f" method="post" action="/submit">
                      <input id="q" name="q" value="toy browser">
                      <input type="checkbox" name="exact" value="1" checked>
                      <input name="ignored" value="x" disabled>
                      <button id="go" type="submit">Save</button>
                    </form>"#
                    .to_vec(),
            )),
            _ => Err(LoadError::NotFound(url.to_string())),
        }
    }

    fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        self.requests.lock().unwrap().push(request.clone());

        if request.url.path() == "/submit" && request.method == Method::Post {
            return Ok(FetchResponse::synthetic(
                Url::parse("http://example.test/saved.html").unwrap(),
                200,
                Some("text/html"),
                b"<title>Saved</title><p id=\"result\">stored</p>".to_vec(),
            ));
        }

        Ok(FetchResponse::synthetic(
            request.url.clone(),
            404,
            Some("text/plain"),
            b"not found".to_vec(),
        ))
    }
}

#[test]
fn post_form_sends_urlencoded_body_and_navigates_to_response() {
    let loader = RecordingLoader::default();
    let probe = loader.clone();
    let start = Url::parse("http://example.test/form.html").unwrap();
    let mut browser = Browser::open(Box::new(loader), &start).expect("form page opens");

    let button = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();
    let outcome = browser.click_node(&button);

    assert!(matches!(outcome, ClickOutcome::Navigated(_)), "{outcome:?}");
    assert_eq!(browser.url().to_string(), "http://example.test/saved.html");
    assert_eq!(browser.document().title().as_deref(), Some("Saved"));
    assert_eq!(browser.history().len(), 2);
    assert!(browser.can_go_back());

    let requests = probe.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method, Method::Post);
    assert_eq!(request.url.to_string(), "http://example.test/submit");
    assert_eq!(
        request.headers.get("content-type").as_deref(),
        Some("application/x-www-form-urlencoded; charset=UTF-8")
    );
    assert_eq!(
        request.body.as_deref(),
        Some(b"q=toy+browser&exact=1".as_slice())
    );
}

#[derive(Clone, Default)]
struct RejectingLoader;

impl ResourceLoader for RejectingLoader {
    fn load(&self, url: &Url) -> Result<Resource, LoadError> {
        if url.path() == "/form.html" {
            return Ok(Resource::new(
                url.clone(),
                Some("text/html".into()),
                br#"<title>Form</title><form method="post" action="/submit"><input name="q" value="v"><button id="go">Save</button></form>"#.to_vec(),
            ));
        }
        Err(LoadError::NotFound(url.to_string()))
    }

    fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        Ok(FetchResponse::synthetic(
            request.url.clone(),
            500,
            Some("text/html"),
            b"<title>Should not render</title>".to_vec(),
        ))
    }
}

#[test]
fn failed_post_keeps_the_current_document_and_history() {
    let start = Url::parse("http://example.test/form.html").unwrap();
    let mut browser = Browser::open(Box::new(RejectingLoader), &start).expect("form page opens");

    let button = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();
    let outcome = browser.click_node(&button);

    match outcome {
        ClickOutcome::NavigationFailed { error, .. } => {
            assert!(matches!(error, LoadError::HttpStatus { status: 500, .. }));
        }
        other => panic!("expected failed navigation, got {other:?}"),
    }
    assert_eq!(browser.url(), &start);
    assert_eq!(browser.document().title().as_deref(), Some("Form"));
    assert_eq!(browser.history().len(), 1);
}
