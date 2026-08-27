use browser_engine::browser::ClickOutcome;
use browser_engine::dom::NodeType;
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

fn set_live_value(browser: &mut Browser, id: &str, value: &str) {
    let path = dom_api::get_element_by_id(&browser.document().dom, id).expect("control");
    let node = dom_api::node_at_mut(&mut browser.document_mut().dom, &path).expect("control node");
    let NodeType::Element(element) = &mut node.node_type else {
        panic!("control is not an element");
    };
    element.set_control_value(value);
}

#[test]
fn invalid_email_blocks_interactive_submission_but_valid_email_navigates() {
    let html = r#"<form action="next">
        <input id="email" type="email" name="email" value="broken">
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    let email = dom_api::get_element_by_id(&browser.document().dom, "email").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&email));

    let html = r#"<form action="next">
        <input type="email" name="email" value="person@example.com">
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();
    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(
        browser.url().to_string(),
        "demo:///next?email=person%40example.com"
    );
}

#[test]
fn number_step_mismatch_blocks_submission_until_the_value_is_on_grid() {
    // The content attribute establishes the step base at 0; changing only the
    // live value lets the test exercise the grid rather than accidentally
    // moving the base together with the value under test.
    let html = r#"<form action="next">
        <input id="qty" type="number" name="qty" step="2" value="0">
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    set_live_value(&mut browser, "qty", "3");
    let qty = dom_api::get_element_by_id(&browser.document().dom, "qty").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&qty));

    let html = r#"<form action="next">
        <input id="qty" type="number" name="qty" step="2" value="0">
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    set_live_value(&mut browser, "qty", "4");
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();
    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(browser.url().to_string(), "demo:///next?qty=4");
}
