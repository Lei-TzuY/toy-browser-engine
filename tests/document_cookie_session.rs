use std::cell::RefCell;
use std::rc::Rc;

use browser_engine::cookie::CookieJar;
use browser_engine::eventloop::ManualClock;
use browser_engine::script::dom_api;
use browser_engine::{Document, MemoryLoader, Url};

fn text(document: &Document, id: &str) -> String {
    let path = dom_api::get_element_by_id(&document.dom, id).expect("element exists");
    dom_api::text_content(dom_api::node_at(&document.dom, &path).unwrap())
}

#[test]
fn inline_bootstrap_script_sees_and_updates_shared_cookie_jar() {
    let url = Url::parse("https://example.test/app/index.html").unwrap();
    let clock = Rc::new(ManualClock::starting_at(10_000.0));
    let mut seeded = CookieJar::with_clock(clock);
    seeded.set_document_cookie("seed=one; Path=/", &url, 0);
    let jar = Rc::new(RefCell::new(seeded));

    let document = Document::from_html_with_session_state(
        r#"
            <div id="seen"></div>
            <script>
                document.getElementById("seen").textContent = document.cookie;
                document.cookie = "written=two; Path=/";
            </script>
        "#,
        &url,
        &MemoryLoader::new(),
        None,
        Some(jar.clone()),
    );

    assert_eq!(text(&document, "seen"), "seed=one");
    let cookies = jar.borrow().get_document_cookie(&url, 0);
    assert!(cookies.contains("seed=one"), "{cookies}");
    assert!(cookies.contains("written=two"), "{cookies}");
    assert!(
        Rc::ptr_eq(&document.runtime.cookie_jar, &jar),
        "the runtime must retain the caller-owned jar rather than a copy"
    );
}

#[test]
fn external_bootstrap_script_uses_the_same_shared_jar() {
    let url = Url::parse("https://example.test/index.html").unwrap();
    let mut loader = MemoryLoader::new();
    loader.insert(
        "https://example.test/app.js",
        r#"
            document.getElementById("seen").textContent = document.cookie;
            document.cookie = "external=yes; Path=/";
        "#,
    );

    let mut seeded = CookieJar::new();
    seeded.set_document_cookie("before=load; Path=/", &url, 0);
    let jar = Rc::new(RefCell::new(seeded));

    let document = Document::from_html_with_session_state(
        r#"<div id="seen"></div><script src="/app.js"></script>"#,
        &url,
        &loader,
        None,
        Some(jar.clone()),
    );

    assert_eq!(text(&document, "seen"), "before=load");
    assert!(
        jar.borrow()
            .get_document_cookie(&url, 0)
            .contains("external=yes")
    );
    assert!(document.diagnostics.is_empty(), "{:?}", document.diagnostics);
}

#[test]
fn session_state_loader_installs_jar_before_scripts_run() {
    let url = Url::parse("https://example.test/index.html").unwrap();
    let mut loader = MemoryLoader::new();
    loader.insert(
        "https://example.test/index.html",
        r#"
            <p id="cookie"></p>
            <script>document.getElementById("cookie").textContent = document.cookie;</script>
        "#,
    );

    let mut seeded = CookieJar::new();
    seeded.set_document_cookie("loaded=yes; Path=/", &url, 0);
    let jar = Rc::new(RefCell::new(seeded));

    let document = Document::load_with_session_state(
        &url,
        &loader,
        None,
        Some(jar.clone()),
    )
    .expect("document loads");

    assert_eq!(text(&document, "cookie"), "loaded=yes");
    assert!(Rc::ptr_eq(&document.runtime.cookie_jar, &jar));
}

#[test]
fn omitting_cookie_session_keeps_standalone_document_isolated() {
    let url = Url::parse("https://example.test/index.html").unwrap();
    let document = Document::from_html_with_session_state(
        r#"
            <p id="cookie"></p>
            <script>
                document.getElementById("cookie").textContent = document.cookie;
                document.cookie = "private=one; Path=/";
            </script>
        "#,
        &url,
        &MemoryLoader::new(),
        None,
        None,
    );

    assert_eq!(text(&document, "cookie"), "");
    assert_eq!(
        document.runtime.cookie_jar.borrow().get_document_cookie(&url, 0),
        "private=one"
    );
}
