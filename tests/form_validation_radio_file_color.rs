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

#[test]
fn readonly_required_file_blocks_until_a_live_value_exists() {
    let mut browser = browser_for(
        r#"<form action="next">
            <input id="upload" type="file" name="upload" required readonly>
            <button id="go">Go</button>
        </form>"#,
    );
    let upload = dom_api::get_element_by_id(&browser.document().dom, "upload").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&upload));

    if let NodeType::Element(element) = &mut dom_api::node_at_mut(&mut browser.document_mut().dom, &upload)
        .unwrap()
        .node_type
    {
        element.set_control_value("picked.txt");
    }

    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    // File controls are not serialized by this engine's urlencoded form-data
    // model yet; the live value here exists only to exercise Required state.
    assert_eq!(browser.url().to_string(), "demo:///next");
}

#[test]
fn required_color_never_blocks_a_raw_empty_control() {
    let mut browser = browser_for(
        r#"<form action="next">
            <input type="color" name="c" required readonly>
            <button id="go">Go</button>
        </form>"#,
    );
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    // Full Color-state sanitization is separate; until then the raw empty value
    // is serialized but must not become a Required-state failure.
    assert_eq!(browser.url().to_string(), "demo:///next?c=");
}

#[test]
fn readonly_radio_uses_group_requiredness_across_explicit_form_owners() {
    let mut browser = browser_for(
        r#"<form id="f" action="next"><button id="go">Go</button></form>
            <input id="a" form="f" type="radio" name="choice" value="a" required readonly>
            <input id="b" form="f" type="radio" name="choice" value="b">"#,
    );
    let a = dom_api::get_element_by_id(&browser.document().dom, "a").unwrap();
    let b = dom_api::get_element_by_id(&browser.document().dom, "b").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&a));

    assert_eq!(browser.click_node(&b), ClickOutcome::Ignored);
    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(browser.url().to_string(), "demo:///next?choice=b");
}
