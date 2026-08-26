use browser_engine::browser::ClickOutcome;
use browser_engine::net::{MemoryLoader, Url};
use browser_engine::script::dom_api;
use browser_engine::Browser;

#[test]
fn form_submit_bypasses_interactive_constraint_validation() {
    let mut loader = MemoryLoader::new();
    loader.insert(
        "demo:///index.html",
        r#"<form id="f" action="done.html">
             <input name="q" required>
             <button id="force" type="button">Force</button>
           </form>
           <script>
             document.getElementById("force").addEventListener("click", function () {
               document.getElementById("f").submit();
             });
           </script>"#,
    );
    loader.insert("demo:///done.html", "<title>Done</title>");

    let start = Url::parse("demo:///index.html").unwrap();
    let mut browser = Browser::open(Box::new(loader), &start).unwrap();
    let force = dom_api::get_element_by_id(&browser.document().dom, "force").unwrap();
    let outcome = browser.click_node(&force);

    assert!(matches!(outcome, ClickOutcome::Navigated(_)), "{outcome:?}");
    assert_eq!(browser.url().to_string(), "demo:///done.html?q=");
}
