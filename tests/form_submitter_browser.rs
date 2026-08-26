use std::sync::{Arc, Mutex};

use browser_engine::browser::ClickOutcome;
use browser_engine::input::{Key, KeyEvent};
use browser_engine::net::{
    FetchError, FetchRequest, FetchResponse, Method, Resource, Url,
};
use browser_engine::script::dom_api;
use browser_engine::{Browser, ResourceLoader};

#[test]
fn clicked_submitter_overrides_get_destination_and_contributes_value() {
    let mut loader = browser_engine::MemoryLoader::new();
    loader.insert(
        "demo:///editor.html",
        r#"<form action="save" method="post">
             <input name="title" value="Toy Browser">
             <button id="preview" name="intent" value="preview"
                     formaction="preview" formmethod="get">Preview</button>
             <button id="save" name="intent" value="save">Save</button>
           </form>"#,
    );
    loader.insert("demo:///preview", "<title>Preview</title>");

    let mut browser = Browser::open(
        Box::new(loader),
        &Url::parse("demo:///editor.html").unwrap(),
    )
    .unwrap();
    let preview = dom_api::get_element_by_id(&browser.document().dom, "preview").unwrap();

    let outcome = browser.click_node(&preview);
    assert!(matches!(outcome, ClickOutcome::Navigated(_)), "{outcome:?}");
    assert_eq!(
        browser.url().to_string(),
        "demo:///preview?title=Toy+Browser&intent=preview"
    );
}

#[test]
fn formnovalidate_bypasses_required_but_normal_submitter_does_not() {
    let mut loader = browser_engine::MemoryLoader::new();
    loader.insert(
        "demo:///editor.html",
        r#"<form action="next">
             <input id="title" name="title" required>
             <button id="normal">Publish</button>
             <button id="draft" formnovalidate name="intent" value="draft">Draft</button>
           </form>"#,
    );
    loader.insert("demo:///next", "<title>Next</title>");

    let mut browser = Browser::open(
        Box::new(loader),
        &Url::parse("demo:///editor.html").unwrap(),
    )
    .unwrap();
    let normal = dom_api::get_element_by_id(&browser.document().dom, "normal").unwrap();
    assert_eq!(browser.click_node(&normal), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///editor.html");

    let draft = dom_api::get_element_by_id(&browser.document().dom, "draft").unwrap();
    let outcome = browser.click_node(&draft);
    assert!(matches!(outcome, ClickOutcome::Navigated(_)), "{outcome:?}");
    assert_eq!(browser.url().to_string(), "demo:///next?intent=draft");
}

#[test]
fn enter_uses_the_first_enabled_submitter() {
    let mut loader = browser_engine::MemoryLoader::new();
    loader.insert(
        "demo:///editor.html",
        r#"<form action="save">
             <input id="q" name="q" value="hello">
             <button disabled formaction="wrong">Wrong</button>
             <button id="preview" name="intent" value="preview" formaction="preview">Preview</button>
             <button name="intent" value="save">Save</button>
           </form>"#,
    );
    loader.insert("demo:///preview", "<title>Preview</title>");

    let mut browser = Browser::open(
        Box::new(loader),
        &Url::parse("demo:///editor.html").unwrap(),
    )
    .unwrap();
    let field = dom_api::get_element_by_id(&browser.document().dom, "q").unwrap();
    browser.document_mut().focus_path(&field);

    let outcome = browser.press_key(&KeyEvent::new(Key::Enter));
    assert!(matches!(outcome, ClickOutcome::Navigated(_)), "{outcome:?}");
    assert_eq!(
        browser.url().to_string(),
        "demo:///preview?q=hello&intent=preview"
    );
}

#[test]
fn enter_validates_before_the_submit_event() {
    let mut loader = browser_engine::MemoryLoader::new();
    loader.insert(
        "demo:///editor.html",
        r#"<form id="f" action="next">
             <input id="q" name="q" required>
             <button>Go</button>
           </form>
           <script>
             document.getElementById("q").addEventListener("invalid", function () {
                 console.log("invalid");
             });
             document.getElementById("f").addEventListener("submit", function () {
                 console.log("submit");
             });
           </script>"#,
    );
    loader.insert("demo:///next", "<title>Next</title>");

    let mut browser = Browser::open(
        Box::new(loader),
        &Url::parse("demo:///editor.html").unwrap(),
    )
    .unwrap();
    browser.document_mut().runtime.quiet = true;
    let field = dom_api::get_element_by_id(&browser.document().dom, "q").unwrap();
    browser.document_mut().focus_path(&field);

    assert_eq!(browser.press_key(&KeyEvent::new(Key::Enter)), ClickOutcome::Script);
    assert_eq!(browser.document().runtime.console, vec!["invalid"]);
    assert_eq!(browser.url().to_string(), "demo:///editor.html");
}

#[derive(Clone)]
struct RecordingLoader {
    requests: Arc<Mutex<Vec<FetchRequest>>>,
}

impl RecordingLoader {
    fn new() -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl ResourceLoader for RecordingLoader {
    fn load(&self, url: &Url) -> Result<Resource, browser_engine::net::LoadError> {
        if url.to_string() == "http://example.test/editor" {
            return Ok(Resource {
                url: url.clone(),
                bytes: br#"<form action="/save" method="get">
                    <input name="title" value="Toy Browser">
                    <button id="save" name="intent" value="save" formmethod="post">Save</button>
                </form>"#
                    .to_vec(),
                content_type: Some("text/html".into()),
            });
        }
        Err(browser_engine::net::LoadError::NotFound(url.to_string()))
    }

    fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        self.requests.lock().unwrap().push(request.clone());
        Ok(FetchResponse::synthetic(
            Url::parse("http://example.test/saved").unwrap(),
            200,
            Some("text/html"),
            b"<title>Saved</title>".to_vec(),
        ))
    }
}

#[test]
fn submitter_formmethod_post_reaches_the_transport_with_submitter_payload() {
    let loader = RecordingLoader::new();
    let requests = loader.requests.clone();
    let mut browser = Browser::open(
        Box::new(loader),
        &Url::parse("http://example.test/editor").unwrap(),
    )
    .unwrap();
    let save = dom_api::get_element_by_id(&browser.document().dom, "save").unwrap();

    let outcome = browser.click_node(&save);
    assert!(matches!(outcome, ClickOutcome::Navigated(_)), "{outcome:?}");

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, Method::Post);
    assert_eq!(requests[0].url.to_string(), "http://example.test/save");
    assert_eq!(
        String::from_utf8_lossy(requests[0].body.as_deref().unwrap()),
        "title=Toy+Browser&intent=save"
    );
}
