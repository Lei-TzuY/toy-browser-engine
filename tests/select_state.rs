use browser_engine::browser::ClickOutcome;
use browser_engine::net::Url;
use browser_engine::script::dom_api;
use browser_engine::select_state;
use browser_engine::{Browser, MemoryLoader};

fn browser_for(html: &str) -> Browser {
    let mut loader = MemoryLoader::new();
    loader.insert("demo:///form.html", html);
    loader.insert("demo:///next", "<title>Next</title>");
    Browser::open(Box::new(loader), &Url::parse("demo:///form.html").unwrap()).unwrap()
}

#[test]
fn setting_select_value_changes_the_submitted_query() {
    let html = r#"<form action="next">
        <select id="pick" name="pick">
            <option value="a" selected>A</option>
            <option value="b">B</option>
        </select>
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    let pick = dom_api::get_element_by_id(&browser.document().dom, "pick").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert!(select_state::set_value(
        &mut browser.document_mut().dom,
        &pick,
        "b"
    ));
    assert_eq!(
        select_state::value(&browser.document().dom, &pick).as_deref(),
        Some("b")
    );
    assert!(matches!(
        browser.click_node(&go),
        ClickOutcome::Navigated(_)
    ));
    assert_eq!(browser.url().to_string(), "demo:///next?pick=b");
}

#[test]
fn unknown_value_clears_required_select_and_blocks_submission() {
    let html = r#"<form action="next">
        <select id="pick" name="pick" required>
            <option value="a" selected>A</option>
            <option value="b">B</option>
        </select>
        <button id="go">Go</button>
    </form>"#;
    let mut browser = browser_for(html);
    let pick = dom_api::get_element_by_id(&browser.document().dom, "pick").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert!(select_state::set_value(
        &mut browser.document_mut().dom,
        &pick,
        "missing"
    ));
    assert_eq!(
        select_state::selected_index(&browser.document().dom, &pick),
        Some(-1)
    );
    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&pick));
}
