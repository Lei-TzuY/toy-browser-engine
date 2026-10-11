use browser_engine::document::Document;
use browser_engine::net::{MemoryLoader, Url};

fn run_js(js: &str) -> Document {
    let loader = MemoryLoader::new();
    let url = Url::parse("http://example.com/app/index.html").unwrap();
    let full_html = format!(
        "<!DOCTYPE html><html><body><div id=\"parent\"><button id=\"btn\">Click me</button></div><script>{}</script></body></html>",
        js
    );
    Document::from_html(&full_html, &url, &loader)
}

#[test]
fn test_custom_event_dispatch_and_bubbling_with_detail() {
    let doc = run_js(r#"
        var parentEl = document.getElementById("parent");
        var btnEl = document.getElementById("btn");

        parentEl.addEventListener("user-action", function(e) {
            console.log("received:" + e.detail.action + ":" + e.bubbles);
        });

        var event = new CustomEvent("user-action", {
            detail: { action: "login" },
            bubbles: true,
            cancelable: true
        });

        btnEl.dispatchEvent(event);
    "#);

    let logs = doc.runtime.console;
    assert_eq!(logs.first().cloned().unwrap_or_default(), "received:login:true");
}
