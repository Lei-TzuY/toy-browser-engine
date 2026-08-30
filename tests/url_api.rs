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
fn test_url_constructor_and_properties() {
    let doc = run_js(
        r##"
        let u = new URL("https://example.com:8080/path/to/page?query=1&foo=bar#section1");
        console.log("href:" + u.href);
        console.log("protocol:" + u.protocol);
        console.log("origin:" + u.origin);
        console.log("hostname:" + u.hostname);
        console.log("port:" + u.port);
        console.log("pathname:" + u.pathname);
        console.log("search:" + u.search);
        console.log("hash:" + u.hash);

        // Relative URL with base
        let rel = new URL("/sub/api", "https://example.org:3000/main");
        console.log("rel_href:" + rel.href);
    "##,
    );

    let logs = doc.runtime.console;
    assert_eq!(
        logs[0],
        "href:https://example.com:8080/path/to/page?query=1&foo=bar#section1"
    );
    assert_eq!(logs[1], "protocol:https:");
    assert_eq!(logs[2], "origin:https://example.com:8080");
    assert_eq!(logs[3], "hostname:example.com");
    assert_eq!(logs[4], "port:8080");
    assert_eq!(logs[5], "pathname:/path/to/page");
    assert_eq!(logs[6], "search:?query=1&foo=bar");
    assert_eq!(logs[7], "hash:#section1");
    assert_eq!(logs[8], "rel_href:https://example.org:3000/sub/api");
}

#[test]
fn test_url_property_mutations() {
    let doc = run_js(
        r##"
        let u = new URL("http://example.com/start");
        u.pathname = "/new/path";
        u.search = "?a=b&c=d";
        u.hash = "#heading";
        console.log("mutated:" + u.href);
    "##,
    );

    let logs = doc.runtime.console;
    assert_eq!(
        logs[0],
        "mutated:http://example.com/new/path?a=b&c=d#heading"
    );
}

#[test]
fn test_url_search_params_methods_and_reactive_sync() {
    let doc = run_js(
        r##"
        let params = new URLSearchParams("key1=val1&key2=val2&key1=val3");
        console.log("get key1:" + params.get("key1"));
        console.log("has key2:" + params.has("key2"));
        console.log("getAll key1:" + JSON.stringify(params.getAll("key1")));
        console.log("size:" + params.size);

        params.append("key1", "val4");
        params.set("key2", "updated");
        params.delete("key3");
        console.log("toString:" + params.toString());

        // Test reactive sync with parent URL
        let u = new URL("https://example.com/api?lang=en&version=1");
        let sp = u.searchParams;
        sp.set("lang", "zh-TW");
        sp.append("theme", "dark");
        console.log("sync_search:" + u.search);
        console.log("sync_href:" + u.href);
    "##,
    );

    let logs = doc.runtime.console;
    assert_eq!(logs[0], "get key1:val1");
    assert_eq!(logs[1], "has key2:true");
    assert_eq!(logs[2], r#"getAll key1:["val1","val3"]"#);
    assert_eq!(logs[3], "size:3");
    assert_eq!(
        logs[4],
        "toString:key1=val1&key2=updated&key1=val3&key1=val4"
    );
    assert_eq!(logs[5], "sync_search:?lang=zh-TW&version=1&theme=dark");
    assert_eq!(
        logs[6],
        "sync_href:https://example.com/api?lang=zh-TW&version=1&theme=dark"
    );
}
