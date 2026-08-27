use browser_engine::browser::ClickOutcome;
use browser_engine::net::{MemoryLoader, Url};
use browser_engine::script::dom_api;
use browser_engine::Browser;

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
fn number_ignores_character_length_constraints_in_editing_and_submission() {
    let mut browser = browser_for(
        r#"<form action="next">
            <input id="qty" type="number" name="qty" minlength="5" maxlength="2">
            <button id="go">Go</button>
        </form>"#,
    );
    let qty = dom_api::get_element_by_id(&browser.document().dom, "qty").unwrap();
    browser.document_mut().focus_path(&qty);

    browser.type_text("1234");

    let value = dom_api::node_at(&browser.document().dom, &qty)
        .and_then(|node| node.as_element())
        .map(|element| element.control_value())
        .expect("number input");
    assert_eq!(value, "1234", "maxlength must not truncate Number state");

    let validity = browser_engine::validation::control_validity(&browser.document().dom, &qty);
    assert!(!validity.too_short);
    assert!(!validity.too_long);
    assert!(validity.valid(), "unexpected Number validity: {validity:?}");

    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();
    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(browser.url().to_string(), "demo:///next?qty=1234");
}

#[test]
fn text_input_still_honors_maxlength_after_number_fix() {
    let mut browser = browser_for(
        r#"<form action="next">
            <input id="q" name="q" maxlength="2">
            <button id="go">Go</button>
        </form>"#,
    );
    let q = dom_api::get_element_by_id(&browser.document().dom, "q").unwrap();
    browser.document_mut().focus_path(&q);

    browser.type_text("abcd");

    let value = dom_api::node_at(&browser.document().dom, &q)
        .and_then(|node| node.as_element())
        .map(|element| element.control_value())
        .expect("text input");
    assert_eq!(value, "ab");

    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();
    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(browser.url().to_string(), "demo:///next?q=ab");
}
