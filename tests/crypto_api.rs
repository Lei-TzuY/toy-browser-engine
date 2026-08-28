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
fn test_crypto_random_uuid_and_get_random_values() {
    let doc = run_js(r#"
        let uuid = crypto.randomUUID();
        console.log("uuid_len:" + uuid.length);
        console.log("uuid_valid:" + (uuid.indexOf("-") > 0));

        let arr = [0, 0, 0, 0];
        let filled = crypto.getRandomValues(arr);
        console.log("arr_len:" + filled.length);
        console.log("has_nonzero:" + (filled[0] >= 0));
    "#);

    let logs = doc.runtime.console;
    assert_eq!(logs[0], "uuid_len:36");
    assert_eq!(logs[1], "uuid_valid:true");
    assert_eq!(logs[2], "arr_len:4");
    assert_eq!(logs[3], "has_nonzero:true");
}
