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
fn malformed_number_lexemes_block_interactive_submission_and_receive_focus() {
    for value in [" 12 ", "+12", "12."] {
        let html = format!(
            r#"<form action="next">
                <input id="qty" type="number" name="qty" value="{value}">
                <button id="go">Go</button>
            </form>"#
        );
        let mut browser = browser_for(&html);
        let qty = dom_api::get_element_by_id(&browser.document().dom, "qty").unwrap();
        let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

        assert_eq!(
            browser.click_node(&go),
            ClickOutcome::Script,
            "{value:?} must be rejected"
        );
        assert_eq!(browser.url().to_string(), "demo:///form.html");
        assert_eq!(browser.document().focused_path().as_ref(), Some(&qty));
    }
}

#[test]
fn valid_html_number_lexemes_still_submit_their_raw_value() {
    for (value, expected) in [
        (".5", "demo:///next?qty=.5"),
        ("-.5", "demo:///next?qty=-.5"),
        ("1e+2", "demo:///next?qty=1e%2B2"),
    ] {
        let html = format!(
            r#"<form action="next">
                <input type="number" name="qty" value="{value}">
                <button id="go">Go</button>
            </form>"#
        );
        let mut browser = browser_for(&html);
        let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

        assert!(
            matches!(browser.click_node(&go), ClickOutcome::Navigated(_)),
            "{value:?} must remain valid"
        );
        assert_eq!(browser.url().to_string(), expected);
    }
}
