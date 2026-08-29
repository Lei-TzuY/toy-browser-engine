use browser_engine::document::Document;
use browser_engine::net::{MemoryLoader, Url};

fn run_js(js: &str) -> Document {
    let loader = MemoryLoader::new();
    let url = Url::parse("http://example.com/app/index.html").unwrap();
    let full_html = format!(
        "<!DOCTYPE html><html><body><script>{}</script></body></html>",
        js
    );
    Document::from_html(&full_html, &url, &loader)
}

#[test]
fn test_abort_signal_timeout_and_any() {
    let doc = run_js(r#"
        // Test AbortSignal.abort()
        const s1 = AbortSignal.abort();
        console.log("s1:" + s1.aborted);

        // Test AbortSignal.timeout(0)
        const s2 = AbortSignal.timeout(0);
        console.log("s2:" + s2.aborted);

        // Test AbortSignal.any()
        const s3 = AbortSignal.any([s1]);
        console.log("s3:" + s3.aborted);

        // Test un-aborted controller signal in any()
        const c4 = new AbortController();
        const s4 = AbortSignal.any([c4.signal]);
        console.log("s4_before:" + s4.aborted);
        c4.abort();
        const s5 = AbortSignal.any([c4.signal]);
        console.log("s5_after:" + s5.aborted);
    "#);

    let logs = doc.runtime.console;
    assert_eq!(logs[0], "s1:true");
    assert_eq!(logs[1], "s2:true");
    assert_eq!(logs[2], "s3:true");
    assert_eq!(logs[3], "s4_before:false");
    assert_eq!(logs[4], "s5_after:true");
}
