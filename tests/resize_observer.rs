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
fn test_resize_observer_lifecycle_and_records() {
    let doc = run_js(
        r#"
        let target = document.getElementById("target");
        let ro = new ResizeObserver(function(entries) {});

        ro.observe(target);
        let records = ro.takeRecords();
        console.log("records_len:" + records.length);
        if (records.length > 0) {
            let entry = records[0];
            console.log("target_id:" + entry.target);
            console.log("content_rect_w:" + entry.contentRect.width);
        }

        ro.unobserve(target);
        console.log("records_after_unobserve:" + ro.takeRecords().length);

        ro.observe(target);
        ro.disconnect();
        console.log("records_after_disconnect:" + ro.takeRecords().length);
    "#,
    );

    let logs = doc.runtime.console;
    assert_eq!(logs[0], "records_len:1");
    assert_eq!(logs[1], "target_id:[object HTMLElement]");
    assert_eq!(logs[2], "content_rect_w:100");
    assert_eq!(logs[3], "records_after_unobserve:0");
    assert_eq!(logs[4], "records_after_disconnect:0");
}
