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
fn readonly_checkbox_does_not_escape_required_validation() {
    let mut browser = browser_for(
        r#"
        <form action="next">
            <input id="agree" type="checkbox" name="agree" value="yes" required readonly>
            <button id="go">Go</button>
        </form>
        "#,
    );
    let agree = dom_api::get_element_by_id(&browser.document().dom, "agree").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&agree));

    // `readonly` is inapplicable to checkbox state, so ordinary activation is
    // still allowed to satisfy the required constraint.
    let _ = browser.click_node(&agree);
    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(browser.url().to_string(), "demo:///next?agree=yes");
}

#[test]
fn readonly_select_still_blocks_on_its_required_placeholder() {
    let mut browser = browser_for(
        r#"
        <form action="next">
            <select id="pick" name="pick" required readonly>
                <option value="">Choose</option>
                <option value="x">X</option>
            </select>
            <button id="go">Go</button>
        </form>
        "#,
    );
    let pick = dom_api::get_element_by_id(&browser.document().dom, "pick").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&pick));
}

#[test]
fn required_attribute_on_range_and_color_never_blocks_submission() {
    let mut browser = browser_for(
        r#"
        <form action="next">
            <input type="range" name="r" required>
            <input type="color" name="c" required>
            <button id="go">Go</button>
        </form>
        "#,
    );
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(browser.url().to_string(), "demo:///next?r=&c=");
}
