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
fn number_minlength_and_maxlength_do_not_block_submission() {
    let mut browser = browser_for(
        r#"<form action="next">
            <input id="qty" type="number" name="qty" value="12" minlength="3" maxlength="1">
            <button id="go">Go</button>
        </form>"#,
    );
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert!(matches!(
        browser.click_node(&go),
        ClickOutcome::Navigated(_)
    ));
    assert_eq!(browser.url().to_string(), "demo:///next?qty=12");
}

#[test]
fn number_range_constraints_still_apply_when_length_attributes_are_present() {
    let mut browser = browser_for(
        r#"<form action="next">
            <input id="qty" type="number" name="qty" value="12" minlength="3" maxlength="9" min="20">
            <button id="go">Go</button>
        </form>"#,
    );
    let qty = dom_api::get_element_by_id(&browser.document().dom, "qty").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&qty));
}
