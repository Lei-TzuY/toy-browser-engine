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
fn range_min_max_and_step_block_interactive_submission() {
    let html = r#"<form action="next">
        <input id="level" type="range" name="level" min="10" max="20" step="2" value="15">
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    let level = dom_api::get_element_by_id(&browser.document().dom, "level").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&level));

    let html = r#"<form action="next">
        <input type="range" name="level" min="10" max="20" step="2" value="16">
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();
    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(browser.url().to_string(), "demo:///next?level=16");
}

#[test]
fn range_uses_default_bounds_and_ignores_readonly_for_validation() {
    let html = r#"<form action="next">
        <input id="level" type="range" name="level" readonly value="101">
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    let level = dom_api::get_element_by_id(&browser.document().dom, "level").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&level));
}

#[test]
fn malformed_range_raw_value_blocks_but_html_number_syntax_submits() {
    let html = r#"<form action="next">
        <input id="level" type="range" name="level" min="0" max="200" step="any" value="+12">
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    let level = dom_api::get_element_by_id(&browser.document().dom, "level").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&level));

    for value in [".5", "-.5", "1e+2"] {
        let html = format!(
            r#"<form action="next">
                <input type="range" name="level" min="-10" max="200" step="any" value="{value}">
                <button id="go">Go</button>
            </form>"#
        );
        let mut browser = browser_for(&html);
        let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();
        assert!(
            matches!(browser.click_node(&go), ClickOutcome::Navigated(_)),
            "{value:?} should submit"
        );
        assert_eq!(
            browser.url().to_string(),
            format!("demo:///next?level={}", value.replace('+', "%2B"))
        );
    }
}
