use browser_engine::document::Document;
use browser_engine::net::{MemoryLoader, Url};

fn run_js(html: &str, js: &str) -> Document {
    let loader = MemoryLoader::new();
    let url = Url::parse("http://example.com/index.html").unwrap();
    let full_html = format!(
        "<!DOCTYPE html><html><body>{}<script>{}</script></body></html>",
        html, js
    );
    Document::from_html(&full_html, &url, &loader)
}

#[test]
fn test_dom_parser_parse_from_string() {
    let doc = run_js(
        r#"<div id="container"></div>"#,
        r##"
            let parser = new DOMParser();
            let parsedDoc = parser.parseFromString("<html><body><div id='box' class='item'><span id='label'>Hello World</span></div></body></html>", "text/html");

            let boxEl = parsedDoc.getElementById("box");
            let labelEl = parsedDoc.querySelector("#label");
            let bodyEl = parsedDoc.body;

            console.log("has_box:" + (boxEl ? "yes" : "no"));
            console.log("box_class:" + (boxEl ? boxEl.getAttribute("class") : "none"));
            console.log("label_text:" + (labelEl ? labelEl.textContent : "none"));
            console.log("has_body:" + (bodyEl ? "yes" : "no"));
        "##,
    );

    let logs = doc.runtime.console;
    assert_eq!(logs[0], "has_box:yes");
    assert_eq!(logs[1], "box_class:item");
    assert_eq!(logs[2], "label_text:Hello World");
    assert_eq!(logs[3], "has_body:yes");
}

#[test]
fn test_xml_serializer_serialize_to_string() {
    let doc = run_js(
        r#"<div id="card" class="widget"><h2 class="title">My Card</h2><p>Description</p></div>"#,
        r##"
            let serializer = new XMLSerializer();
            let card = document.getElementById("card");
            let htmlStr = serializer.serializeToString(card);

            console.log("serialized:" + htmlStr);
        "##,
    );

    let logs = doc.runtime.console;
    assert!(logs[0].contains("<div"));
    assert!(logs[0].contains("id=\"card\""));
    assert!(logs[0].contains("class=\"widget\""));
    assert!(logs[0].contains("My Card"));
}

#[test]
fn test_dom_parser_and_append_to_live_dom() {
    let doc = run_js(
        r#"<div id="root"></div>"#,
        r##"
            let parser = new DOMParser();
            let external = parser.parseFromString("<div id='injected'><span>Dynamic Content</span></div>", "text/html");

            let injectedDiv = external.getElementById("injected");
            let root = document.getElementById("root");
            root.appendChild(injectedDiv);

            let liveInjected = document.getElementById("injected");
            console.log("live_found:" + (liveInjected ? "yes" : "no"));
            console.log("live_text:" + (liveInjected ? liveInjected.textContent : "none"));
        "##,
    );

    let logs = doc.runtime.console;
    assert_eq!(logs[0], "live_found:yes");
    assert_eq!(logs[1], "live_text:Dynamic Content");
}
