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
fn malformed_time_blocks_submission_and_focuses_the_control() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <input id="slot" type="time" name="slot" value="24:00">
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
fn ordinary_time_range_blocks_values_outside_the_interval() {
    for value in ["08:59", "17:01"] {
        let mut browser = browser_for(&format!(
            r#"
            <form action="next">
              <input id="slot" type="time" name="slot" value="{value}"
                     min="09:00" max="17:00">
              <button id="go">Go</button>
            </form>
            "#
        ));
        let slot = dom_api::get_element_by_id(&browser.document().dom, "slot").unwrap();
        let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

        assert_eq!(browser.click_node(&go), ClickOutcome::Script);
        assert_eq!(browser.url().to_string(), "demo:///form.html");
        assert_eq!(browser.document().focused_path().as_ref(), Some(&slot));
    }
}

#[test]
fn reversed_time_range_wraps_across_midnight() {
    for value in ["21:00", "23:30", "00:00", "06:00"] {
        let mut browser = browser_for(&format!(
            r#"
            <form action="next">
              <input type="time" name="slot" value="{value}"
                     min="21:00" max="06:00" step="any">
              <button id="go">Go</button>
            </form>
            "#
        ));
        let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

        assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
        assert_eq!(
            browser.url().to_string(),
            format!("demo:///next?slot={}", value.replace(':', "%3A"))
        );
    }
}

#[test]
fn reversed_time_range_rejects_the_daytime_gap() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <input id="slot" type="time" name="slot" value="12:00"
                 min="21:00" max="06:00" step="any">
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
fn default_time_step_uses_midnight_as_base_and_sixty_seconds_as_step() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <input id="slot" type="time" name="slot">
          <button id="go">Go</button>
        </form>
        <script>document.getElementById("slot").value = "00:00:30";</script>
        "#,
    );
    let slot = dom_api::get_element_by_id(&browser.document().dom, "slot").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&slot));
}

#[test]
fn second_and_fractional_second_steps_submit_exact_live_strings() {
    for (value, step) in [("12:34:56", "1"), ("12:34:56.500", "0.5")] {
        let mut browser = browser_for(&format!(
            r#"
            <form action="next">
              <input type="time" name="slot" value="{value}" step="{step}">
              <button id="go">Go</button>
            </form>
            "#
        ));
        let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

        assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
        assert_eq!(
            browser.url().to_string(),
            format!("demo:///next?slot={}", value.replace(':', "%3A"))
        );
    }
}

#[test]
fn invalid_time_bounds_are_ignored_and_step_any_allows_fractional_seconds() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <input type="time" name="slot" value="12:34:56.789"
                 min="99:00" max="not-a-time" step="any">
          <button id="go">Go</button>
        </form>
        "#,
    );
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(
        browser.url().to_string(),
        "demo:///next?slot=12%3A34%3A56.789"
    );
}
