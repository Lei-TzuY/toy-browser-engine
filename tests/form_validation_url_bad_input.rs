use browser_engine::browser::ClickOutcome;
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
fn url_type_mismatch_blocks_submission_until_the_value_is_absolute() {
    let html = r#"<form action="next">
        <input id="target" type="url" name="target" value="/relative/path">
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    let target = dom_api::get_element_by_id(&browser.document().dom, "target").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&target));

    let html = r#"<form action="next">
        <input type="url" name="target" value="https://example.com">
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();
    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(
        browser.url().to_string(),
        "demo:///next?target=https%3A%2F%2Fexample.com"
    );
}

#[test]
fn malformed_number_bad_input_blocks_interactive_submission() {
    let html = r#"<form action="next">
        <input id="qty" type="number" name="qty" value="abc">
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    let qty = dom_api::get_element_by_id(&browser.document().dom, "qty").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&qty));

    let html = r#"<form action="next">
        <input type="number" name="qty" value="12.5">
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();
    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(browser.url().to_string(), "demo:///next?qty=12.5");
}
