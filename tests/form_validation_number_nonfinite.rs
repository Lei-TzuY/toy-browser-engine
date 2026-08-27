use browser_engine::browser::ClickOutcome;
use browser_engine::dom::NodeType;
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
fn overflowing_number_blocks_submission_and_focuses_the_control() {
    let mut browser = browser_for(
        r#"<form action="next">
            <input id="qty" type="number" name="qty" value="1e999">
            <button id="go">Go</button>
        </form>"#,
    );
    let qty = dom_api::get_element_by_id(&browser.document().dom, "qty").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    let validity = browser_engine::validation::control_validity(&browser.document().dom, &qty);
    assert!(validity.bad_input);
    assert!(!validity.valid());

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&qty));
}

#[test]
fn replacing_overflow_with_a_finite_value_restores_submission() {
    let mut browser = browser_for(
        r#"<form action="next">
            <input id="qty" type="number" name="qty" value="1e999">
            <button id="go">Go</button>
        </form>"#,
    );
    let qty = dom_api::get_element_by_id(&browser.document().dom, "qty").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    let node = dom_api::node_at_mut(&mut browser.document_mut().dom, &qty).unwrap();
    let NodeType::Element(element) = &mut node.node_type else {
        panic!("number input is not an element");
    };
    element.set_control_value("42");

    let validity = browser_engine::validation::control_validity(&browser.document().dom, &qty);
    assert!(validity.valid(), "unexpected Number validity: {validity:?}");

    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(browser.url().to_string(), "demo:///next?qty=42");
}
