use browser_engine::document::Document;
use browser_engine::net::{MemoryLoader, Url};

fn run_js(html: &str, js: &str) -> Document {
    let loader = MemoryLoader::new();
    let url = Url::parse("http://example.com/index.html").unwrap();
    let full_html = format!(
        "<!DOCTYPE html><html><head><style>:root {{ --main-bg: #123456; --accent: red; }} .box {{ display: flex; font-size: 20px; color: var(--accent); background-color: var(--main-bg); }}</style></head><body>{}<script>{}</script></body></html>",
        html, js
    );
    Document::from_html(&full_html, &url, &loader)
}

#[test]
fn test_get_computed_style_basic_and_custom_props() {
    let doc = run_js(
        r#"<div id="target" class="box"><span>Inner</span></div>"#,
        r#"
            let el = document.getElementById("target");
            let style = window.getComputedStyle(el);

            let disp = style.getPropertyValue("display");
            let fs = style.getPropertyValue("font-size");
            let col = style.getPropertyValue("color");
            let bg = style.getPropertyValue("background-color");
            let customMain = style.getPropertyValue("--main-bg");
            let customAccent = style.getPropertyValue("--accent");

            console.log("display:" + disp);
            console.log("font-size:" + fs);
            console.log("color:" + col);
            console.log("bg:" + bg);
            console.log("main-bg:" + customMain);
            console.log("accent:" + customAccent);
        "#,
    );

    let logs = doc.runtime.console;
    assert_eq!(logs[0], "display:flex");
    assert_eq!(logs[1], "font-size:20px");
    assert_eq!(logs[2], "color:rgb(255, 0, 0)");
    assert_eq!(logs[3], "bg:rgb(18, 52, 86)");
    assert_eq!(logs[4], "main-bg:#123456");
    assert_eq!(logs[5], "accent:red");
}

#[test]
fn test_get_computed_style_camel_case_and_live_mutation() {
    let doc = run_js(
        r#"<div id="target" style="display: block; color: green;"></div>"#,
        r#"
            let el = document.getElementById("target");
            let s1 = getComputedStyle(el);

            console.log("init-color:" + s1.color);
            console.log("init-display:" + s1.display);

            // Mutate inline style
            el.style.color = "rgb(0, 0, 255)";
            el.style.display = "inline-block";

            let s2 = getComputedStyle(el);
            console.log("after-color:" + s2.color);
            console.log("after-display:" + s2.display);
        "#,
    );

    let logs = doc.runtime.console;
    assert_eq!(logs[0], "init-color:rgb(0, 128, 0)");
    assert_eq!(logs[1], "init-display:block");
    assert_eq!(logs[2], "after-color:rgb(0, 0, 255)");
    assert_eq!(logs[3], "after-display:inline-block");
}

#[test]
fn test_window_global_properties() {
    let doc = run_js(
        r#"<div id="d"></div>"#,
        r#"
            console.log("self_is_window:" + (window.self === window ? "yes" : "no"));
            console.log("win_has_doc:" + (window.document ? "yes" : "no"));
            console.log("win_width:" + window.innerWidth);
            console.log("win_dpr:" + window.devicePixelRatio);
        "#,
    );

    let logs = doc.runtime.console;
    assert_eq!(logs[0], "self_is_window:yes");
    assert_eq!(logs[1], "win_has_doc:yes");
    assert_eq!(logs[2], "win_width:800");
    assert_eq!(logs[3], "win_dpr:1");
}
