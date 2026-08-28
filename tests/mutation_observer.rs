use browser_engine::document::Document;
use browser_engine::net::{MemoryLoader, Url};

fn run_js(js: &str) -> Document {
    let loader = MemoryLoader::new();
    let url = Url::parse("http://example.com/app/index.html").unwrap();
    let full_html = format!(
        r#"<!DOCTYPE html><html><body><div id="target"></div><script>{}</script></body></html>"#,
        js
    );
    Document::from_html(&full_html, &url, &loader)
}

#[test]
fn test_mutation_observer_observe_and_take_records() {
    let doc = run_js(r#"
        let target = document.getElementById("target");
        let observer = new MutationObserver(function(mutations) {});
        observer.observe(target, { childList: true, attributes: true });
        
        let records = observer.takeRecords();
        console.log("records_len:" + records.length);
        if (records.length > 0) {
            console.log("record_type:" + records[0].type);
        }
        
        observer.disconnect();
        console.log("records_after_disconnect:" + observer.takeRecords().length);
    "#);

    let logs = doc.runtime.console;
    assert_eq!(logs[0], "records_len:1");
    assert_eq!(logs[1], "record_type:childList");
    assert_eq!(logs[2], "records_after_disconnect:0");
}
