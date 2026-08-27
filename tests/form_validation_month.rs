use browser_engine::browser::{Browser, ClickOutcome};
use browser_engine::net::Url;
use browser_engine::script::dom_api;
use browser_engine::MemoryLoader;

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
fn malformed_month_blocks_submission_and_focuses_the_control() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <input id="billing" type="month" name="billing" value="2026-13">
          <button id="go">Go</button>
        </form>
        "#,
    );
    let billing = dom_api::get_element_by_id(&browser.document().dom, "billing").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&billing));
}

#[test]
fn month_range_and_step_constraints_block_submission() {
    for html in [
        r#"
        <form action="next">
          <input id="billing" type="month" name="billing" value="2026-08" min="2026-09">
          <button id="go">Go</button>
        </form>
        "#,
        r#"
        <form action="next">
          <input id="billing" type="month" name="billing" value="2026-05" min="2026-01" step="3">
          <button id="go">Go</button>
        </form>
        "#,
    ] {
        let mut browser = browser_for(html);
        let billing = dom_api::get_element_by_id(&browser.document().dom, "billing").unwrap();
        let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

        assert_eq!(browser.click_node(&go), ClickOutcome::Script);
        assert_eq!(browser.url().to_string(), "demo:///form.html");
        assert_eq!(browser.document().focused_path().as_ref(), Some(&billing));
    }
}

#[test]
fn valid_month_submits_and_serializes_the_value() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <input type="month" name="billing" value="2026-07" min="2026-01" max="2026-12" step="3">
          <button id="go">Go</button>
        </form>
        "#,
    );
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(browser.url().to_string(), "demo:///next?billing=2026-07");
}

#[test]
fn invalid_min_and_max_are_ignored_and_step_any_submits() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <input type="month" name="billing" value="2026-08"
                 min="nope" max="2026-99" step="any">
          <button id="go">Go</button>
        </form>
        "#,
    );
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(browser.url().to_string(), "demo:///next?billing=2026-08");
}
