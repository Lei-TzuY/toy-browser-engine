use std::rc::Rc;
use std::time::Duration;

use browser_engine::css::parser::{Color, Value};
use browser_engine::eventloop::ManualClock;
use browser_engine::net::{ManualNetwork, MemoryLoader, Url};
use browser_engine::script::dom_api;
use browser_engine::{Browser, PointerState};

fn url(input: &str) -> Url {
    Url::parse(input).expect("valid URL")
}

fn target_attr(browser: &Browser, name: &str) -> Option<String> {
    let path = dom_api::get_element_by_id(&browser.document().dom, "target")?;
    let element = dom_api::node_at(&browser.document().dom, &path)?.as_element()?;
    element.get_attr(name).map(str::to_string)
}

fn target_color(browser: &Browser) -> Option<Value> {
    let styled = browser
        .document()
        .style_tree(800.0, &PointerState::default());
    fn find(node: &browser_engine::style::StyledNode<'_>) -> Option<Value> {
        if node
            .node
            .as_element()
            .and_then(|element| element.get_attr("id"))
            == Some("target")
        {
            return node.value("color").cloned();
        }
        node.children.iter().find_map(find)
    }
    find(&styled)
}

#[test]
fn timer_inserted_external_script_runs_before_advance_time_returns() {
    let page = "https://page.test/index.html";
    let mut loader = MemoryLoader::new();
    loader.insert(
        page,
        r#"
            <div id="target"></div><div id="host"></div>
            <script>
              setTimeout(function () {
                const script = document.createElement("script");
                script.setAttribute("src", "/late.js");
                document.getElementById("host").appendChild(script);
              }, 10);
            </script>
        "#,
    );
    loader.insert(
        "https://page.test/late.js",
        r#"document.getElementById("target").setAttribute("data-timer-script", "loaded");"#,
    );

    let clock = Rc::new(ManualClock::new());
    let mut browser = Browser::open_with_clock(Box::new(loader), &url(page), clock)
        .expect("page loads");
    assert_eq!(target_attr(&browser, "data-timer-script"), None);

    let report = browser.advance_time(Duration::from_millis(10));
    assert_eq!(report.timers_run, 1);
    assert_eq!(
        target_attr(&browser, "data-timer-script").as_deref(),
        Some("loaded"),
        "the timer-created script must execute in the same browser turn"
    );
}

#[test]
fn animation_frame_inserted_stylesheet_reaches_the_next_paint() {
    let page = "https://page.test/index.html";
    let mut loader = MemoryLoader::new();
    loader.insert(
        page,
        r#"
            <style>#target { color: rgb(1, 2, 3); }</style>
            <p id="target">x</p><div id="host"></div>
            <script>
              requestAnimationFrame(function () {
                const link = document.createElement("link");
                link.setAttribute("rel", "stylesheet");
                link.setAttribute("href", "/late.css");
                document.getElementById("host").appendChild(link);
              });
            </script>
        "#,
    );
    loader.insert(
        "https://page.test/late.css",
        "#target { color: rgb(7, 8, 9); }",
    );

    let mut browser = Browser::open(Box::new(loader), &url(page)).expect("page loads");
    assert_eq!(target_color(&browser), Some(Value::Color(Color::rgb(1, 2, 3))));

    let report = browser.tick();
    assert_eq!(report.frames_run, 1);
    assert_eq!(
        target_color(&browser),
        Some(Value::Color(Color::rgb(7, 8, 9))),
        "stylesheet inserted by requestAnimationFrame must be active before paint"
    );
}

#[test]
fn fetch_promise_inserted_script_runs_in_the_completion_turn() {
    let page = "https://page.test/index.html";
    let mut loader = MemoryLoader::new();
    loader.insert(
        page,
        r#"
            <div id="target"></div><div id="host"></div>
            <script>
              fetch("/gate").then(function () {
                const script = document.createElement("script");
                script.setAttribute("src", "/after-fetch.js");
                document.getElementById("host").appendChild(script);
              });
            </script>
        "#,
    );
    loader.insert(
        "https://page.test/after-fetch.js",
        r#"document.getElementById("target").setAttribute("data-fetch-script", "loaded");"#,
    );

    let transport = Rc::new(ManualNetwork::new());
    transport.respond_text("https://page.test/gate", "ok");
    let mut browser = Browser::open_with_network(
        Box::new(loader),
        transport.clone(),
        &url(page),
        Rc::new(ManualClock::new()),
    )
    .expect("page loads");

    let sent = browser.tick();
    assert_eq!(sent.requests_sent, 1);
    assert_eq!(target_attr(&browser, "data-fetch-script"), None);
    assert_eq!(transport.complete_all(), 1);

    let completed = browser.tick();
    assert_eq!(completed.network_completions, 1);
    assert_eq!(
        target_attr(&browser, "data-fetch-script").as_deref(),
        Some("loaded"),
        "promise reaction-created script must execute before the completion turn returns"
    );
}
