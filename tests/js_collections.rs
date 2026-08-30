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
fn test_js_map_operations() {
    let doc = run_js(
        r#"
        let m = new Map([["a", 1], ["b", 2]]);
        console.log("init_size:" + m.size);
        console.log("get_a:" + m.get("a"));
        console.log("has_b:" + m.has("b"));

        m.set("c", 3);
        console.log("after_set_size:" + m.size);
        console.log("get_c:" + m.get("c"));

        m.delete("b");
        console.log("after_delete_size:" + m.size);
        console.log("has_b_after:" + m.has("b"));

        console.log("keys_len:" + m.keys().length);
        console.log("values_len:" + m.values().length);
        console.log("entries_len:" + m.entries().length);

        m.clear();
        console.log("after_clear_size:" + m.size);
    "#,
    );

    let logs = doc.runtime.console;
    assert_eq!(logs[0], "init_size:2");
    assert_eq!(logs[1], "get_a:1");
    assert_eq!(logs[2], "has_b:true");
    assert_eq!(logs[3], "after_set_size:3");
    assert_eq!(logs[4], "get_c:3");
    assert_eq!(logs[5], "after_delete_size:2");
    assert_eq!(logs[6], "has_b_after:false");
    assert_eq!(logs[7], "keys_len:2");
    assert_eq!(logs[8], "values_len:2");
    assert_eq!(logs[9], "entries_len:2");
    assert_eq!(logs[10], "after_clear_size:0");
}

#[test]
fn test_js_set_operations() {
    let doc = run_js(
        r#"
        let s = new Set(["apple", "banana", "apple"]);
        console.log("init_size:" + s.size);
        console.log("has_apple:" + s.has("apple"));
        console.log("has_orange:" + s.has("orange"));

        s.add("orange");
        console.log("after_add_size:" + s.size);
        console.log("has_orange_after:" + s.has("orange"));

        s.delete("banana");
        console.log("after_delete_size:" + s.size);
        console.log("has_banana_after:" + s.has("banana"));

        console.log("keys_len:" + s.keys().length);
        console.log("values_len:" + s.values().length);

        s.clear();
        console.log("after_clear_size:" + s.size);
    "#,
    );

    let logs = doc.runtime.console;
    assert_eq!(logs[0], "init_size:2");
    assert_eq!(logs[1], "has_apple:true");
    assert_eq!(logs[2], "has_orange:false");
    assert_eq!(logs[3], "after_add_size:3");
    assert_eq!(logs[4], "has_orange_after:true");
    assert_eq!(logs[5], "after_delete_size:2");
    assert_eq!(logs[6], "has_banana_after:false");
    assert_eq!(logs[7], "keys_len:2");
    assert_eq!(logs[8], "values_len:2");
    assert_eq!(logs[9], "after_clear_size:0");
}
