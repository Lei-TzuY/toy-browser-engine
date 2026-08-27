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
fn test_element_matches() {
    let doc = run_js(
        r#"<div id="target" class="card active" data-role="admin"><span>Text</span></div>"#,
        r#"
            let el = document.getElementById("target");
            console.log("m1:" + (el.matches("div") ? "yes" : "no"));
            console.log("m2:" + (el.matches(".card.active") ? "yes" : "no"));
            console.log("m3:" + (el.matches("[data-role='admin']") ? "yes" : "no"));
            console.log("m4:" + (el.matches("span") ? "yes" : "no"));
            console.log("m5:" + (el.matches(".inactive") ? "yes" : "no"));
        "#,
    );

    let logs = doc.runtime.console;
    assert_eq!(logs[0], "m1:yes");
    assert_eq!(logs[1], "m2:yes");
    assert_eq!(logs[2], "m3:yes");
    assert_eq!(logs[3], "m4:no");
    assert_eq!(logs[4], "m5:no");
}

#[test]
fn test_element_closest() {
    let doc = run_js(
        r#"<div class="container" id="c1">
            <section class="panel" id="p1">
                <button class="btn" id="b1"><span>Click</span></button>
            </section>
        </div>"#,
        r#"
            let btn = document.getElementById("b1");
            let span = btn.querySelector("span");

            let closestBtn = span.closest(".btn");
            let closestPanel = span.closest(".panel");
            let closestContainer = span.closest(".container");
            let closestNonExistent = span.closest(".missing");

            console.log("b:" + (closestBtn ? closestBtn.getAttribute("id") : "none"));
            console.log("p:" + (closestPanel ? closestPanel.getAttribute("id") : "none"));
            console.log("c:" + (closestContainer ? closestContainer.getAttribute("id") : "none"));
            console.log("m:" + (closestNonExistent ? "found" : "none"));
        "#,
    );

    let logs = doc.runtime.console;
    assert_eq!(logs[0], "b:b1");
    assert_eq!(logs[1], "p:p1");
    assert_eq!(logs[2], "c:c1");
    assert_eq!(logs[3], "m:none");
}

#[test]
fn test_element_dataset_bidirectional() {
    let doc = run_js(
        r#"<div id="target" data-user-id="42" data-first-name="Alice"></div>"#,
        r#"
            let el = document.getElementById("target");
            let uid = el.dataset.userId;
            let fname = el.dataset.firstName;

            // Set new property via dataset
            el.dataset.themeMode = "dark";
            el.dataset.lastName = "Smith";

            console.log("uid:" + uid);
            console.log("fname:" + fname);
            console.log("theme:" + el.getAttribute("data-theme-mode"));
            console.log("lname:" + el.getAttribute("data-last-name"));
        "#,
    );

    let logs = doc.runtime.console;
    assert_eq!(logs[0], "uid:42");
    assert_eq!(logs[1], "fname:Alice");
    assert_eq!(logs[2], "theme:dark");
    assert_eq!(logs[3], "lname:Smith");
}

#[test]
fn test_node_clone_node_and_contains() {
    let doc = run_js(
        r#"<div id="parent"><div id="child"><span id="grandchild">Hello</span></div></div>"#,
        r#"
            let parent = document.getElementById("parent");
            let child = document.getElementById("child");
            let grandchild = document.getElementById("grandchild");

            // contains
            let p_contains_c = parent.contains(child);
            let p_contains_gc = parent.contains(grandchild);
            let p_contains_p = parent.contains(parent);
            let c_contains_p = child.contains(parent);

            console.log("p_c:" + (p_contains_c ? "yes" : "no"));
            console.log("p_gc:" + (p_contains_gc ? "yes" : "no"));
            console.log("p_p:" + (p_contains_p ? "yes" : "no"));
            console.log("c_p:" + (c_contains_p ? "yes" : "no"));

            // cloneNode
            let deepClone = child.cloneNode(true);
            let shallowClone = child.cloneNode(false);

            console.log("deep_has_span:" + (deepClone.querySelector("span") ? "yes" : "no"));
            console.log("shallow_has_span:" + (shallowClone.querySelector("span") ? "yes" : "no"));
        "#,
    );

    let logs = doc.runtime.console;
    assert_eq!(logs[0], "p_c:yes");
    assert_eq!(logs[1], "p_gc:yes");
    assert_eq!(logs[2], "p_p:yes");
    assert_eq!(logs[3], "c_p:no");
    assert_eq!(logs[4], "deep_has_span:yes");
    assert_eq!(logs[5], "shallow_has_span:no");
}
