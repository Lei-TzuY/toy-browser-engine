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
    let html = r#"<form action="next">
        <input id="qty" type="number" name="qty" step="2" value="3">
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    let qty = dom_api::get_element_by_id(&browser.document().dom, "qty").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&qty));

    let html = r#"<form action="next">
        <input type="number" name="qty" step="2" value="4">
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();
    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(browser.url().to_string(), "demo:///next?qty=4");
}
