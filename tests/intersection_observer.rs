use browser_engine::document::Document;
use browser_engine::net::{MemoryLoader, Url};

fn run_js(js: &str) -> Document {
    let loader = MemoryLoader::new();
    let url = Url::parse("http://example.com/app/index.html").unwrap();
    let full_html = format!(
        "<!DOCTYPE html><html><body><div id=\"target1\"></div><script>{}</script></body></html>",
        js
    );
    Document::from_html(&full_html, &url, &loader)
}

#[test]
fn test_intersection_observer_lifecycle() {
    let doc = run_js(
        r#"
        let observer = new IntersectionObserver((entries) => {}, {
            threshold: [0.0, 0.5, 1.0]
        });

        console.log("rootMargin:" + observer.rootMargin);
        console.log("thresholds_len:" + observer.thresholds.length);

        observer.observe("target1");
        let records = observer.takeRecords();
        console.log("records_len:" + records.length);
        if (records.length > 0) {
            console.log("target:" + records[0].target);
            console.log("isIntersecting:" + records[0].isIntersecting);
            console.log("intersectionRatio:" + records[0].intersectionRatio);
        }

        observer.unobserve("target1");
        let after_unobserve = observer.takeRecords();
        console.log("after_unobserve_len:" + after_unobserve.length);

        observer.observe("target1");
        observer.disconnect();
        let after_disconnect = observer.takeRecords();
        console.log("after_disconnect_len:" + after_disconnect.length);
    "#,
    );

    let logs = doc.runtime.console;
    assert_eq!(logs[0], "rootMargin:0px");
    assert_eq!(logs[1], "thresholds_len:3");
    assert_eq!(logs[2], "records_len:1");
    assert_eq!(logs[3], "target:target1");
    assert_eq!(logs[4], "isIntersecting:true");
    assert_eq!(logs[5], "intersectionRatio:1");
    assert_eq!(logs[6], "after_unobserve_len:0");
    assert_eq!(logs[7], "after_disconnect_len:0");
}
