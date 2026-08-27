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
fn malformed_datetime_local_blocks_submission_and_focuses_the_control() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <input id="when" type="datetime-local" name="when" value="2026-02-30T12:00">
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
fn datetime_local_range_blocks_values_on_either_side_across_dates() {
    for value in ["2026-08-26T23:59", "2026-08-28T00:01"] {
        let mut browser = browser_for(&format!(
            r#"
            <form action="next">
              <input id="when" type="datetime-local" name="when" value="{value}"
                     min="2026-08-27T00:00" max="2026-08-28T00:00">
              <button id="go">Go</button>
            </form>
            "#
        ));
        let when = dom_api::get_element_by_id(&browser.document().dom, "when").unwrap();
        let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

        assert_eq!(browser.click_node(&go), ClickOutcome::Script);
        assert_eq!(browser.url().to_string(), "demo:///form.html");
        assert_eq!(browser.document().focused_path().as_ref(), Some(&when));
    }
}

#[test]
fn default_datetime_local_step_uses_epoch_and_sixty_seconds() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <input id="when" type="datetime-local" name="when">
          <button id="go">Go</button>
        </form>
        <script>document.getElementById("when").value = "1970-01-01T00:00:30";</script>
        "#,
    );
    let when = dom_api::get_element_by_id(&browser.document().dom, "when").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&when));
}

#[test]
fn datetime_local_step_grid_can_span_a_day_boundary() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <input id="when" type="datetime-local" name="when"
                 value="2026-08-28T00:15" min="2026-08-27T23:30" step="3600">
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
fn valid_datetime_local_with_fractional_seconds_submits() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <input type="datetime-local" name="when"
                 value="2026-08-27T12:34:56.500" step="0.5">
          <button id="go">Go</button>
        </form>
        "#,
    );
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(
        browser.url().to_string(),
        "demo:///next?when=2026-08-27T12%3A34%3A56.500"
    );
}

#[test]
fn valid_space_separator_is_parseable_in_the_raw_value_model() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <input type="datetime-local" name="when" value="2026-08-27 12:34" step="any">
          <button id="go">Go</button>
        </form>
        "#,
    );
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(
        browser.url().to_string(),
        "demo:///next?when=2026-08-27+12%3A34"
    );
}

#[test]
fn invalid_datetime_local_bounds_are_ignored_and_step_any_submits() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <input type="datetime-local" name="when" value="2026-08-27T12:34:56.789"
                 min="2026-02-30T00:00" max="not-a-datetime" step="any">
          <button id="go">Go</button>
        </form>
        "#,
    );
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(
        browser.url().to_string(),
        "demo:///next?when=2026-08-27T12%3A34%3A56.789"
    );
}
