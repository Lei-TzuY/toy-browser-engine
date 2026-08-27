use browser_engine::browser::Browser;
use browser_engine::document::Document;
use browser_engine::net::{MemoryLoader, Url};

#[test]
fn test_local_storage_basic_methods() {
    let html = r#"
    <html>
        <body>
            <script>
                localStorage.clear();
                console.log("initial-len:" + localStorage.length);

                localStorage.setItem("user", "alice");
                localStorage.setItem("theme", "dark");
                console.log("after-set-len:" + localStorage.length);
                console.log("get-user:" + localStorage.getItem("user"));
                console.log("get-theme:" + localStorage.getItem("theme"));
                console.log("get-missing:" + localStorage.getItem("missing"));

                console.log("key-0:" + localStorage.key(0));
                console.log("key-1:" + localStorage.key(1));
                console.log("key-2:" + localStorage.key(2));

                localStorage.removeItem("user");
                console.log("after-remove-len:" + localStorage.length);
                console.log("after-remove-user:" + localStorage.getItem("user"));

                localStorage.clear();
                console.log("after-clear-len:" + localStorage.length);
            </script>
        </body>
    </html>
    "#;

    let loader = MemoryLoader::new();
    let url = Url::parse("http://example.com/").unwrap();
    let doc = Document::from_html(html, &url, &loader);

    let logs = doc.runtime.console.clone();
    assert_eq!(logs[0], "initial-len:0");
    assert_eq!(logs[1], "after-set-len:2");
    assert_eq!(logs[2], "get-user:alice");
    assert_eq!(logs[3], "get-theme:dark");
    assert_eq!(logs[4], "get-missing:null");
    assert_eq!(logs[5], "key-0:user");
    assert_eq!(logs[6], "key-1:theme");
    assert_eq!(logs[7], "key-2:null");
    assert_eq!(logs[8], "after-remove-len:1");
    assert_eq!(logs[9], "after-remove-user:null");
    assert_eq!(logs[10], "after-clear-len:0");
}

#[test]
fn test_session_storage_isolation() {
    let html = r#"
    <html>
        <body>
            <script>
                sessionStorage.clear();
                localStorage.clear();

                sessionStorage.setItem("sessionKey", "sessionVal");
                localStorage.setItem("localKey", "localVal");

                console.log("session:" + sessionStorage.getItem("sessionKey"));
                console.log("session-in-local:" + localStorage.getItem("sessionKey"));
                console.log("local:" + localStorage.getItem("localKey"));
                console.log("local-in-session:" + sessionStorage.getItem("localKey"));
            </script>
        </body>
    </html>
    "#;

    let loader = MemoryLoader::new();
    let url = Url::parse("http://example.com/").unwrap();
    let doc = Document::from_html(html, &url, &loader);

    let logs = doc.runtime.console.clone();
    assert_eq!(logs[0], "session:sessionVal");
    assert_eq!(logs[1], "session-in-local:null");
    assert_eq!(logs[2], "local:localVal");
    assert_eq!(logs[3], "local-in-session:null");
}

#[test]
fn test_storage_property_and_index_access() {
    let html = r#"
    <html>
        <body>
            <script>
                localStorage.clear();
                localStorage.name = "bob";
                localStorage["mode"] = "compact";

                console.log("prop-name:" + localStorage.name);
                console.log("index-mode:" + localStorage["mode"]);
                console.log("method-name:" + localStorage.getItem("name"));
                console.log("method-mode:" + localStorage.getItem("mode"));
                console.log("length:" + localStorage.length);
            </script>
        </body>
    </html>
    "#;

    let loader = MemoryLoader::new();
    let url = Url::parse("http://example.com/").unwrap();
    let doc = Document::from_html(html, &url, &loader);

    let logs = doc.runtime.console.clone();
    assert_eq!(logs[0], "prop-name:bob");
    assert_eq!(logs[1], "index-mode:compact");
    assert_eq!(logs[2], "method-name:bob");
    assert_eq!(logs[3], "method-mode:compact");
    assert_eq!(logs[4], "length:2");
}

#[test]
fn test_browser_origin_scoped_local_storage_persistence() {
    let mut loader = MemoryLoader::new();

    let page1_url = Url::parse("http://app.example.com/page1.html").unwrap();
    let page2_url = Url::parse("http://app.example.com/page2.html").unwrap();
    let other_origin_url = Url::parse("http://other.org/index.html").unwrap();

    loader.insert(
        "http://app.example.com/page1.html",
        r#"
        <html><body><script>
            localStorage.setItem("authToken", "secret123");
            sessionStorage.setItem("tempData", "temp123");
        </script></body></html>
        "#,
    );

    loader.insert(
        "http://app.example.com/page2.html",
        r#"
        <html><body><script>
            console.log("page2-token:" + localStorage.getItem("authToken"));
            console.log("page2-session:" + sessionStorage.getItem("tempData"));
        </script></body></html>
        "#,
    );

    loader.insert(
        "http://other.org/index.html",
        r#"
        <html><body><script>
            console.log("other-origin-token:" + localStorage.getItem("authToken"));
        </script></body></html>
        "#,
    );

    let mut browser = Browser::open(Box::new(loader), &page1_url).unwrap();

    // Navigate to page 2 (same origin)
    browser.navigate(&page2_url).unwrap();
    let logs_page2 = browser.document().runtime.console.clone();
    assert_eq!(logs_page2[0], "page2-token:secret123");
    assert_eq!(logs_page2[1], "page2-session:null");

    // Navigate to other origin
    browser.navigate(&other_origin_url).unwrap();
    let logs_other = browser.document().runtime.console.clone();
    assert_eq!(logs_other[0], "other-origin-token:null");
}
