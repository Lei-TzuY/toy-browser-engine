use browser_engine::net::{MemoryLoader, Url};
use browser_engine::script::dom_api;
use browser_engine::Browser;

#[test]
fn invalid_event_is_non_bubbling_and_precedes_submit() {
    let mut loader = MemoryLoader::new();
    loader.insert(
        "demo:///index.html",
        r#"<form id="f" action="done.html">
             <input id="q" name="q" required>
             <button id="go">Go</button>
           </form>
           <script>
             document.getElementById("q").addEventListener("invalid", function () {
               console.log("invalid-control");
             });
             document.getElementById("f").addEventListener("invalid", function () {
               console.log("invalid-form");
             });
             document.getElementById("f").addEventListener("submit", function () {
               console.log("submit");
             });
           </script>"#,
    );
    loader.insert("demo:///done.html", "<title>Done</title>");

    let start = Url::parse("demo:///index.html").unwrap();
    let mut browser = Browser::open(Box::new(loader), &start).unwrap();
    browser.document_mut().runtime.quiet = true;
    let button = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    browser.click_node(&button);

    assert_eq!(browser.document().runtime.console, vec!["invalid-control"]);
    assert_eq!(browser.url(), &start);
}
