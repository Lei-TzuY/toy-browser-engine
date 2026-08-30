use browser_engine::browser::ClickOutcome;
use browser_engine::script::dom_api;
use browser_engine::{Browser, MemoryLoader, Url};

#[test]
fn submit_listener_can_change_the_submitter_destination_before_navigation() {
    let mut loader = MemoryLoader::new();
    loader.insert(
        "demo:///editor.html",
        r#"<form id="f" action="old">
             <input name="q" value="live">
             <button id="go" name="intent" value="save">Save</button>
           </form>
           <script>
             document.getElementById("f").addEventListener("submit", function () {
                 document.getElementById("go").setAttribute("formaction", "new");
             });
           </script>"#,
    );
    loader.insert("demo:///new", "<title>New target</title>");

    let mut browser = Browser::open(
        Box::new(loader),
        &Url::parse("demo:///editor.html").unwrap(),
    )
    .unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    let outcome = browser.click_node(&go);
    assert!(matches!(outcome, ClickOutcome::Navigated(_)), "{outcome:?}");
    assert_eq!(browser.url().to_string(), "demo:///new?q=live&intent=save");
}
