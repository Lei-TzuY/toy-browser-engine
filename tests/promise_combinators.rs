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
fn test_promise_race_any_and_all_settled() {
    let doc = run_js(r#"
        // 1. Test Promise.race
        const p1 = Promise.resolve("first");
        const p2 = Promise.resolve("second");
        Promise.race([p1, p2]).then(val => {
            console.log("race:" + val);
        });

        // 2. Test Promise.allSettled
        const p3 = Promise.resolve(42);
        const p4 = Promise.reject("err");
        Promise.allSettled([p3, p4]).then(results => {
            console.log("settled_0_status:" + results[0].status);
            console.log("settled_0_val:" + results[0].value);
            console.log("settled_1_status:" + results[1].status);
            console.log("settled_1_reason:" + results[1].reason);
        });

        // 3. Test Promise.any
        const p5 = Promise.reject("fail1");
        const p6 = Promise.resolve("win");
        Promise.any([p5, p6]).then(val => {
            console.log("any:" + val);
        });
    "#);

    let logs = doc.runtime.console;
    assert_eq!(logs[0], "race:first");
    assert_eq!(logs[1], "settled_0_status:fulfilled");
    assert_eq!(logs[2], "settled_0_val:42");
    assert_eq!(logs[3], "settled_1_status:rejected");
    assert_eq!(logs[4], "settled_1_reason:err");
    assert_eq!(logs[5], "any:win");
}
