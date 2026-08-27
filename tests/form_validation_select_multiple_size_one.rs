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
fn required_multiple_size_one_rejects_its_selected_placeholder_label_option() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <select id="pick" name="pick" required multiple size="1">
            <option value="" selected>Choose</option>
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
