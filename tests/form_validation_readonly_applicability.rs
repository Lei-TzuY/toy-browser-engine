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
fn readonly_required_checkbox_still_participates_in_validation() {
    let html = r#"<form action="next">
        <input id="agree" type="checkbox" name="agree" required readonly>
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    let agree = dom_api::get_element_by_id(&browser.document().dom, "agree").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&agree));

    // `readonly` is inapplicable to Checkbox state, so user activation still
    // toggles checkedness. `ClickOutcome::Ignored` only means there was no
    // listener or navigation; the control's default action still ran.
    assert_eq!(browser.click_node(&agree), ClickOutcome::Ignored);
    let checked = dom_api::node_at(&browser.document().dom, &agree)
        .and_then(|node| node.as_element())
        .is_some_and(|element| element.is_checked());
    assert!(checked);

    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(browser.url().to_string(), "demo:///next?agree=on");
}

#[test]
fn readonly_required_select_still_blocks_on_its_placeholder() {
    let html = r#"<form action="next">
        <select id="pick" name="pick" required readonly>
            <option value="">Choose</option>
            <option value="x">X</option>
        </select>
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    let pick = dom_api::get_element_by_id(&browser.document().dom, "pick").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&pick));
}

#[test]
fn readonly_required_radio_is_not_a_validation_escape_hatch() {
    let html = r#"<form action="next">
        <input id="a" type="radio" name="choice" value="a" required readonly>
        <input id="b" type="radio" name="choice" value="b">
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    let a = dom_api::get_element_by_id(&browser.document().dom, "a").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.document().focused_path().as_ref(), Some(&a));
}

#[test]
fn readonly_text_input_remains_barred_from_required_validation() {
    let html = r#"<form action="next">
        <input type="text" name="q" required readonly value="">
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(browser.url().to_string(), "demo:///next?q=");
}
