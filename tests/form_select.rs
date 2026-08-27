use browser_engine::browser::ClickOutcome;
use browser_engine::forms;
use browser_engine::net::Url;
use browser_engine::script::dom_api;
use browser_engine::{Browser, MemoryLoader};

fn browser_for(html: &str) -> Browser {
    let mut loader = MemoryLoader::new();
    loader.insert("demo:///form.html", html);
    loader.insert("demo:///next", "<title>Next</title>");
    Browser::open(
        Box::new(loader),
        &Url::parse("demo:///form.html").unwrap(),
    )
    .unwrap()
}

#[test]
fn browser_get_submission_serializes_single_and_multiple_selects() {
    let html = r#"<form action="next">
        <select name="theme">
            <option value="light">Light</option>
            <option value="dark" selected>Dark</option>
        </select>
        <select name="tag" multiple>
            <option value="rust" selected>Rust</option>
            <option value="ignored">Ignored</option>
            <optgroup disabled><option value="hidden" selected>Hidden</option></optgroup>
            <option selected>  Toy   Browser  </option>
        </select>
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(
        browser.url().to_string(),
        "demo:///next?theme=dark&tag=rust&tag=Toy+Browser"
    );
}

#[test]
fn single_select_without_selected_attribute_uses_first_enabled_option() {
    let html = r#"<form action="next">
        <select name="page">
            <option disabled value="zero">Zero</option>
            <option value="one">One</option>
            <option value="two">Two</option>
        </select>
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(browser.url().to_string(), "demo:///next?page=one");
}

#[test]
fn required_select_placeholder_blocks_submission_until_a_real_value_is_selected() {
    let html = r#"<form action="next">
        <select id="pick" name="pick" required>
            <option value="" selected>Choose one</option>
            <option value="a">A</option>
        </select>
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    let pick = dom_api::get_element_by_id(&browser.document().dom, "pick").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&pick));

    let html = r#"<form action="next">
        <select name="pick" required>
            <option value="">Choose one</option>
            <option value="a" selected>A</option>
        </select>
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(browser.url().to_string(), "demo:///next?pick=a");
}

#[test]
fn live_option_selection_changes_the_browser_submission_payload() {
    let html = r#"<form action="next">
        <select name="pick">
            <option id="a" value="a" selected>A</option>
            <option id="b" value="b">B</option>
        </select>
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    let b = dom_api::get_element_by_id(&browser.document().dom, "b").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert!(forms::set_option_selected(
        &mut browser.document_mut().dom,
        &b,
        true
    ));
    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(browser.url().to_string(), "demo:///next?pick=b");
}

#[test]
fn explicit_live_deselection_makes_required_select_invalid() {
    let html = r#"<form action="next">
        <select id="pick" name="pick" required>
            <option id="a" value="a" selected>A</option>
            <option value="b">B</option>
        </select>
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    let pick = dom_api::get_element_by_id(&browser.document().dom, "pick").unwrap();
    let a = dom_api::get_element_by_id(&browser.document().dom, "a").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert!(forms::set_option_selected(
        &mut browser.document_mut().dom,
        &a,
        false
    ));
    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&pick));
}
