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
fn test_structured_clone_deep_copy() {
    let doc = run_js(r#"
        let original = {
            num: 42,
            str: "hello",
            arr: [1, 2, { nested: "val" }],
            obj: { inner: true }
        };

        let copy = structuredClone(original);

        // Mutate original
        original.num = 100;
        original.arr[0] = 999;
        original.arr[2].nested = "mutated";
        original.obj.inner = false;

        console.log("copy_num:" + copy.num);
        console.log("copy_arr_0:" + copy.arr[0]);
        console.log("copy_nested:" + copy.arr[2].nested);
        console.log("copy_inner:" + copy.obj.inner);
    "#);

    let logs = doc.runtime.console;
    assert_eq!(logs[0], "copy_num:42");
    assert_eq!(logs[1], "copy_arr_0:1");
    assert_eq!(logs[2], "copy_nested:val");
    assert_eq!(logs[3], "copy_inner:true");
}

#[test]
fn test_btoa_and_atob_base64() {
    let doc = run_js(r#"
        let original = "Hello, World!";
        let encoded = btoa(original);
        let decoded = atob(encoded);

        console.log("encoded:" + encoded);
        console.log("decoded:" + decoded);
    "#);

    let logs = doc.runtime.console;
    assert_eq!(logs[0], "encoded:SGVsbG8sIFdvcmxkIQ==");
    assert_eq!(logs[1], "decoded:Hello, World!");
}

#[test]
fn test_request_and_cancel_idle_callback() {
    let doc = run_js(r#"
        let id1 = requestIdleCallback(() => {
            console.log("idle1_ran");
        });

        let id2 = requestIdleCallback(() => {
            console.log("idle2_ran");
        });

        cancelIdleCallback(id2);
        console.log("registered:" + (id1 > 0 && id2 > 0));
    "#);

    let logs = doc.runtime.console;
    assert_eq!(logs[0], "registered:true");
}
