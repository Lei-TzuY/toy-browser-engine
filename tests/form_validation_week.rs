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
fn malformed_iso_week_blocks_submission_and_focuses_the_control() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <input id="slot" type="week" name="slot" value="2021-W53">
          <button id="go">Go</button>
        </form>
        "#,
    );
    let slot = dom_api::get_element_by_id(&browser.document().dom, "slot").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&slot));
}

#[test]
fn week_range_and_cross_year_step_constraints_block_submission() {
    for html in [
        r#"
        <form action="next">
          <input id="slot" type="week" name="slot" value="2025-W51" min="2025-W52">
          <button id="go">Go</button>
        </form>
        "#,
        r#"
        <form action="next">
          <input id="slot" type="week" name="slot" value="2026-W01" min="2025-W52" step="2">
          <button id="go">Go</button>
        </form>
        "#,
    ] {
        let mut browser = browser_for(html);
        let slot = dom_api::get_element_by_id(&browser.document().dom, "slot").unwrap();
        let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

        assert_eq!(browser.click_node(&go), ClickOutcome::Script);
        assert_eq!(browser.url().to_string(), "demo:///form.html");
        assert_eq!(browser.document().focused_path().as_ref(), Some(&slot));
    }
}

#[test]
fn week_default_step_base_is_1970_w01() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <input id="slot" type="week" name="slot" step="2">
          <button id="go">Go</button>
        </form>
        <script>document.getElementById("slot").value = "1970-W02";</script>
        "#,
    );
    let slot = dom_api::get_element_by_id(&browser.document().dom, "slot").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&slot));
}

#[test]
fn valid_week_submits_and_serializes_the_value() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <input type="week" name="slot" value="2026-W02" min="2025-W52" max="2026-W10" step="2">
          <button id="go">Go</button>
        </form>
        "#,
    );
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(browser.url().to_string(), "demo:///next?slot=2026-W02");
}

#[test]
fn invalid_week_bounds_are_ignored_and_step_any_submits() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <input type="week" name="slot" value="2026-W35"
                 min="2021-W53" max="not-a-week" step="any">
          <button id="go">Go</button>
        </form>
        "#,
    );
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(browser.url().to_string(), "demo:///next?slot=2026-W35");
}
