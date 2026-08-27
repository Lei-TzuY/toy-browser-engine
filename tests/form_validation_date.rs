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
fn malformed_date_bad_input_blocks_submission_and_focuses_the_control() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <input id="when" type="date" name="when" value="2026-02-30">
          <button id="go">Go</button>
        </form>
        "#,
    );
    let when = dom_api::get_element_by_id(&browser.document().dom, "when").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&when));
}

#[test]
fn date_range_underflow_blocks_submission() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <input id="when" type="date" name="when" value="2026-08-20"
                 min="2026-08-21" max="2026-08-31">
          <button id="go">Go</button>
        </form>
        "#,
    );
    let when = dom_api::get_element_by_id(&browser.document().dom, "when").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.document().focused_path().as_ref(), Some(&when));
}

#[test]
fn date_step_mismatch_uses_day_units() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <input id="when" type="date" name="when" value="2026-08-22"
                 min="2026-08-21" step="2">
          <button id="go">Go</button>
        </form>
        "#,
    );
    let when = dom_api::get_element_by_id(&browser.document().dom, "when").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.document().focused_path().as_ref(), Some(&when));
}

#[test]
fn valid_date_with_range_and_step_submits_normally() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <input id="when" type="date" name="when" value="2026-08-23"
                 min="2026-08-21" max="2026-08-31" step="2">
          <button id="go">Go</button>
        </form>
        "#,
    );
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    let outcome = browser.click_node(&go);
    assert!(matches!(outcome, ClickOutcome::Navigated(_)), "{outcome:?}");
    assert_eq!(browser.url().to_string(), "demo:///next?when=2026-08-23");
}

#[test]
fn leap_day_is_valid_only_in_a_gregorian_leap_year() {
    let mut valid = browser_for(
        r#"<form action="next"><input type="date" name="when" value="2000-02-29"><button id="go">Go</button></form>"#,
    );
    let go = dom_api::get_element_by_id(&valid.document().dom, "go").unwrap();
    assert!(matches!(valid.click_node(&go), ClickOutcome::Navigated(_)));

    let mut invalid = browser_for(
        r#"<form action="next"><input id="when" type="date" name="when" value="1900-02-29"><button id="go">Go</button></form>"#,
    );
    let when = dom_api::get_element_by_id(&invalid.document().dom, "when").unwrap();
    let go = dom_api::get_element_by_id(&invalid.document().dom, "go").unwrap();
    assert_eq!(invalid.click_node(&go), ClickOutcome::Script);
    assert_eq!(invalid.document().focused_path().as_ref(), Some(&when));
}
