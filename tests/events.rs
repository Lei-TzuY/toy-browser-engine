use browser_engine::document::Document;
use browser_engine::net::{MemoryLoader, Url};

#[test]
fn test_three_phase_event_dispatch_order_and_event_phase() {
    let html = r#"
    <html>
        <body>
            <div id="outer">
                <div id="middle">
                    <button id="inner">Click Me</button>
                </div>
            </div>
            <script>
                var log = [];
                var outer = document.getElementById("outer");
                var middle = document.getElementById("middle");
                var inner = document.getElementById("inner");

                outer.addEventListener("click", function(e) {
                    log.push("outer-capture:" + e.eventPhase);
                }, true);

                middle.addEventListener("click", function(e) {
                    log.push("middle-capture:" + e.eventPhase);
                }, { capture: true });

                inner.addEventListener("click", function(e) {
                    log.push("inner-target:" + e.eventPhase);
                });

                middle.addEventListener("click", function(e) {
                    log.push("middle-bubble:" + e.eventPhase);
                }, false);

                outer.addEventListener("click", function(e) {
                    log.push("outer-bubble:" + e.eventPhase);
                });

                inner.dispatchEvent(new Event("click", { bubbles: true }));
                console.log(log.join(","));
            </script>
        </body>
    </html>
    "#;

    let loader = MemoryLoader::new();
    let url = Url::parse("http://example.com/").unwrap();
    let doc = Document::from_html(html, &url, &loader);

    let output = doc.runtime.console.last().unwrap();
    assert_eq!(
        output,
        "outer-capture:1,middle-capture:1,inner-target:2,middle-bubble:3,outer-bubble:3"
    );
}

#[test]
fn test_stop_propagation() {
    let html = r#"
    <html>
        <body>
            <div id="parent">
                <button id="child">Button</button>
            </div>
            <script>
                var parentHandled = false;
                var childHandled = false;

                document.getElementById("parent").addEventListener("click", function() {
                    parentHandled = true;
                });

                document.getElementById("child").addEventListener("click", function(e) {
                    childHandled = true;
                    e.stopPropagation();
                });

                document.getElementById("child").dispatchEvent(new Event("click", { bubbles: true }));
                console.log("child:" + childHandled + ",parent:" + parentHandled);
            </script>
        </body>
    </html>
    "#;

    let loader = MemoryLoader::new();
    let url = Url::parse("http://example.com/").unwrap();
    let doc = Document::from_html(html, &url, &loader);

    assert_eq!(doc.runtime.console.last().unwrap(), "child:true,parent:false");
}

#[test]
fn test_stop_immediate_propagation() {
    let html = r#"
    <html>
        <body>
            <button id="btn">Click</button>
            <script>
                var count = 0;
                var btn = document.getElementById("btn");

                btn.addEventListener("click", function(e) {
                    count += 1;
                    e.stopImmediatePropagation();
                });

                btn.addEventListener("click", function() {
                    count += 10;
                });

                btn.dispatchEvent(new Event("click", { bubbles: true }));
                console.log("count:" + count);
            </script>
        </body>
    </html>
    "#;

    let loader = MemoryLoader::new();
    let url = Url::parse("http://example.com/").unwrap();
    let doc = Document::from_html(html, &url, &loader);

    assert_eq!(doc.runtime.console.last().unwrap(), "count:1");
}

#[test]
fn test_once_listener_option() {
    let html = r#"
    <html>
        <body>
            <button id="btn">Click</button>
            <script>
                var hits = 0;
                var btn = document.getElementById("btn");

                btn.addEventListener("click", function() {
                    hits += 1;
                }, { once: true });

                btn.dispatchEvent(new Event("click"));
                btn.dispatchEvent(new Event("click"));
                btn.dispatchEvent(new Event("click"));
                console.log("hits:" + hits);
            </script>
        </body>
    </html>
    "#;

    let loader = MemoryLoader::new();
    let url = Url::parse("http://example.com/").unwrap();
    let doc = Document::from_html(html, &url, &loader);

    assert_eq!(doc.runtime.console.last().unwrap(), "hits:1");
}

#[test]
fn test_custom_event_dispatch_with_detail() {
    let html = r#"
    <html>
        <body>
            <div id="container">
                <span id="target"></span>
            </div>
            <script>
                var detailReceived = null;
                var targetType = "";

                document.getElementById("container").addEventListener("user:login", function(e) {
                    detailReceived = e.detail;
                    targetType = e.type;
                });

                var event = new CustomEvent("user:login", {
                    bubbles: true,
                    cancelable: true,
                    detail: { username: "alice", role: "admin" }
                });

                var notPrevented = document.getElementById("target").dispatchEvent(event);
                console.log(targetType + ":" + detailReceived.username + ":" + detailReceived.role + ":" + notPrevented);
            </script>
        </body>
    </html>
    "#;

    let loader = MemoryLoader::new();
    let url = Url::parse("http://example.com/").unwrap();
    let doc = Document::from_html(html, &url, &loader);

    assert_eq!(doc.runtime.console.last().unwrap(), "user:login:alice:admin:true");
}

#[test]
fn test_prevent_default_and_dispatch_return_value() {
    let html = r#"
    <html>
        <body>
            <button id="btn"></button>
            <script>
                var btn = document.getElementById("btn");
                btn.addEventListener("submit", function(e) {
                    e.preventDefault();
                });

                var allowed = btn.dispatchEvent(new Event("submit", { cancelable: true }));
                console.log("allowed:" + allowed);
            </script>
        </body>
    </html>
    "#;

    let loader = MemoryLoader::new();
    let url = Url::parse("http://example.com/").unwrap();
    let doc = Document::from_html(html, &url, &loader);

    assert_eq!(doc.runtime.console.last().unwrap(), "allowed:false");
}
