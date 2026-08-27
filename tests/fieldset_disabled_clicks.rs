use browser_engine::browser::{Browser, ClickOutcome};
use browser_engine::dom::NodeType;
use browser_engine::net::Url;
use browser_engine::script::dom_api;
use browser_engine::MemoryLoader;

fn browser_for(html: &str) -> Browser {
    let mut loader = MemoryLoader::new();
    loader.insert("demo:///form.html", html);
    loader.insert("demo:///next", "<title>Next</title>");
    Browser::open(
        Box::new(loader),
        &Url::parse("demo:///form.html").unwrap(),
    )
    .unwrap()
}

fn element_checked(browser: &Browser, id: &str) -> bool {
    let path = dom_api::get_element_by_id(&browser.document().dom, id).unwrap();
    dom_api::node_at(&browser.document().dom, &path)
        .unwrap()
        .as_element()
        .unwrap()
        .is_checked()
}

fn text(browser: &Browser, id: &str) -> String {
    let path = dom_api::get_element_by_id(&browser.document().dom, id).unwrap();
    dom_api::text_content(dom_api::node_at(&browser.document().dom, &path).unwrap())
}

#[test]
fn inherited_disabled_checkbox_suppresses_focus_event_and_default_action() {
    let mut browser = browser_for(
        r#"
        <fieldset disabled>
          <input id="box" type="checkbox">
        </fieldset>
        <p id="status">idle</p>
        <script>
          document.getElementById("box").addEventListener("click", function () {
            document.getElementById("status").textContent = "clicked";
          });
        </script>
        "#,
    );
    let box_path = dom_api::get_element_by_id(&browser.document().dom, "box").unwrap();

    assert_eq!(browser.click_node(&box_path), ClickOutcome::Ignored);
    assert_eq!(browser.document().focused_path(), None);
    assert!(!element_checked(&browser, "box"));
    assert_eq!(text(&browser, "status"), "idle");
}

#[test]
fn inherited_disabled_submit_button_cannot_dispatch_or_navigate() {
    let mut browser = browser_for(
        r#"
        <form id="f" action="next">
          <input name="q" value="v">
          <fieldset disabled>
            <button id="go" name="intent" value="save">Go</button>
          </fieldset>
        </form>
        <p id="status">idle</p>
        <script>
          document.getElementById("go").addEventListener("click", function () {
            document.getElementById("status").textContent = "clicked";
          });
          document.getElementById("f").addEventListener("submit", function () {
            document.getElementById("status").textContent = "submitted";
          });
        </script>
        "#,
    );
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Ignored);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.history().len(), 1);
    assert_eq!(browser.document().focused_path(), None);
    assert_eq!(text(&browser, "status"), "idle");
}

#[test]
fn first_legend_control_remains_clickable_inside_disabled_fieldset() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <fieldset disabled>
            <legend><button id="go" name="intent" value="legend">Go</button></legend>
            <input name="ignored" value="x">
          </fieldset>
        </form>
        "#,
    );
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    let outcome = browser.click_node(&go);
    assert!(matches!(outcome, ClickOutcome::Navigated(_)), "{outcome:?}");
    assert_eq!(browser.url().to_string(), "demo:///next?intent=legend");
}

#[test]
fn removing_fieldset_disabled_restores_user_click_behavior_live() {
    let mut browser = browser_for(
        r#"
        <fieldset id="group" disabled>
          <input id="box" type="checkbox">
        </fieldset>
        <p id="status">idle</p>
        <script>
          document.getElementById("box").addEventListener("click", function () {
            document.getElementById("status").textContent = "clicked";
          });
        </script>
        "#,
    );
    let group = dom_api::get_element_by_id(&browser.document().dom, "group").unwrap();
    let box_path = dom_api::get_element_by_id(&browser.document().dom, "box").unwrap();

    assert_eq!(browser.click_node(&box_path), ClickOutcome::Ignored);
    assert!(!element_checked(&browser, "box"));

    if let NodeType::Element(element) =
        &mut dom_api::node_at_mut(&mut browser.document_mut().dom, &group)
            .unwrap()
            .node_type
    {
        element.remove_attr("disabled");
    }

    assert_eq!(browser.click_node(&box_path), ClickOutcome::Script);
    assert!(element_checked(&browser, "box"));
    assert_eq!(text(&browser, "status"), "clicked");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&box_path));
}

#[test]
fn directly_disabled_controls_are_suppressed_by_the_same_user_click_gate() {
    let mut browser = browser_for(
        r#"
        <button id="button" disabled>Disabled</button>
        <p id="status">idle</p>
        <script>
          document.getElementById("button").addEventListener("click", function () {
            document.getElementById("status").textContent = "clicked";
          });
        </script>
        "#,
    );
    let button = dom_api::get_element_by_id(&browser.document().dom, "button").unwrap();

    assert_eq!(browser.click_node(&button), ClickOutcome::Ignored);
    assert_eq!(text(&browser, "status"), "idle");
    assert_eq!(browser.document().focused_path(), None);
}
