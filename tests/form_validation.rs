use browser_engine::browser::ClickOutcome;
use browser_engine::input::{Key, KeyEvent};
use browser_engine::net::{MemoryLoader, Url};
use browser_engine::script::dom_api;
use browser_engine::Browser;

fn site() -> MemoryLoader {
    let mut loader = MemoryLoader::new();
    loader.insert(
        "demo:///index.html",
        r#"<title>Validate</title>
           <form id="f" action="done.html">
             <input id="q" name="q" required pattern="[A-Z]{2}[0-9]{2}">
             <button id="go" type="submit">Go</button>
           </form>"#,
    );
    loader.insert("demo:///done.html", "<title>Done</title><p>saved</p>");
    loader
}

#[test]
fn click_blocks_invalid_form_and_focuses_first_control() {
    let start = Url::parse("demo:///index.html").unwrap();
    let mut browser = Browser::open(Box::new(site()), &start).unwrap();

    let button = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();
    let outcome = browser.click_node(&button);

    assert!(!matches!(outcome, ClickOutcome::Navigated(_)), "{outcome:?}");
    assert_eq!(browser.url(), &start);
    assert_eq!(browser.history().len(), 1);

    let focused = browser
        .document()
        .focused_path()
        .expect("first invalid control should receive focus");
    let focused = dom_api::node_at(&browser.document().dom, &focused)
        .and_then(|node| node.as_element())
        .expect("focused element");
    assert_eq!(focused.get_attr("id"), Some("q"));
}

#[test]
fn fixing_constraints_allows_navigation() {
    let start = Url::parse("demo:///index.html").unwrap();
    let mut browser = Browser::open(Box::new(site()), &start).unwrap();

    let field = dom_api::get_element_by_id(&browser.document().dom, "q").unwrap();
    browser.document_mut().focus_path(&field);
    browser.type_text("AB12");

    let button = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();
    let outcome = browser.click_node(&button);

    assert!(matches!(outcome, ClickOutcome::Navigated(_)), "{outcome:?}");
    assert_eq!(browser.url().to_string(), "demo:///done.html?q=AB12");
    assert_eq!(browser.document().title().as_deref(), Some("Done"));
}

#[test]
fn enter_submission_is_blocked_when_invalid() {
    let start = Url::parse("demo:///index.html").unwrap();
    let mut browser = Browser::open(Box::new(site()), &start).unwrap();

    let field = dom_api::get_element_by_id(&browser.document().dom, "q").unwrap();
    browser.document_mut().focus_path(&field);
    let outcome = browser.press_key(&KeyEvent::new(Key::Enter));

    assert!(!matches!(outcome, ClickOutcome::Navigated(_)), "{outcome:?}");
    assert_eq!(browser.url(), &start);
    assert_eq!(browser.history().len(), 1);
}
