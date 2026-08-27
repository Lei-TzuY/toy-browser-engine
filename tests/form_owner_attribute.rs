use browser_engine::browser::{Browser, ClickOutcome};
use browser_engine::dom::NodeType;
use browser_engine::forms;
use browser_engine::net::Url;
use browser_engine::script::dom_api;
use browser_engine::MemoryLoader;

fn browser_for(html: &str) -> Browser {
    let mut loader = MemoryLoader::new();
    loader.insert("demo:///form.html", html);
    loader.insert("demo:///next", "<title>Next</title>");
    loader.insert("demo:///publish", "<title>Publish</title>");
    Browser::open(
        Box::new(loader),
        &Url::parse("demo:///form.html").unwrap(),
    )
    .unwrap()
}

#[test]
fn external_controls_join_form_data_in_document_order() {
    let browser = browser_for(
        r#"
        <input name="before" value="1" form="f">
        <form id="f" action="next">
            <input name="inside" value="2">
        </form>
        <select name="after" form="f"><option value="3" selected>Three</option></select>
        "#,
    );
    let form = dom_api::get_element_by_id(&browser.document().dom, "f").unwrap();

    assert_eq!(
        forms::form_data(&browser.document().dom, &form),
        vec![
            ("before".into(), "1".into()),
            ("inside".into(), "2".into()),
            ("after".into(), "3".into()),
        ]
    );
}

#[test]
fn explicit_form_attribute_reassociates_a_nested_control() {
    let browser = browser_for(
        r#"
        <form id="a"><input id="moved" name="q" value="x" form="b"></form>
        <form id="b" action="next"></form>
        "#,
    );
    let dom = &browser.document().dom;
    let a = dom_api::get_element_by_id(dom, "a").unwrap();
    let b = dom_api::get_element_by_id(dom, "b").unwrap();
    let moved = dom_api::get_element_by_id(dom, "moved").unwrap();

    assert_eq!(forms::owning_form(dom, &moved), Some(b.clone()));
    assert!(forms::form_data(dom, &a).is_empty());
    assert_eq!(forms::form_data(dom, &b), vec![("q".into(), "x".into())]);
}

#[test]
fn invalid_explicit_form_id_does_not_fall_back_to_ancestor_form() {
    let browser = browser_for(
        r#"<form id="a"><input id="orphan" name="q" value="x" form="missing"></form>"#,
    );
    let dom = &browser.document().dom;
    let a = dom_api::get_element_by_id(dom, "a").unwrap();
    let orphan = dom_api::get_element_by_id(dom, "orphan").unwrap();

    assert_eq!(forms::owning_form(dom, &orphan), None);
    assert!(forms::form_data(dom, &a).is_empty());
}

#[test]
fn external_required_control_participates_in_interactive_validation() {
    let mut browser = browser_for(
        r#"
        <form id="f" action="next"><button id="go">Go</button></form>
        <input id="external" name="q" required form="f">
        "#,
    );
    let external = dom_api::get_element_by_id(&browser.document().dom, "external").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&external));
}

#[test]
fn external_submitter_can_submit_and_override_its_form() {
    let mut browser = browser_for(
        r#"
        <form id="f" action="next"><input name="q" value="v"></form>
        <button id="publish" form="f" name="intent" value="publish" formaction="publish">Publish</button>
        "#,
    );
    let publish = dom_api::get_element_by_id(&browser.document().dom, "publish").unwrap();

    assert!(matches!(
        browser.click_node(&publish),
        ClickOutcome::Navigated(_)
    ));
    assert_eq!(
        browser.url().to_string(),
        "demo:///publish?q=v&intent=publish"
    );
}

#[test]
fn external_controls_appear_in_script_form_elements() {
    let mut loader = MemoryLoader::new();
    loader.insert(
        "demo:///form.html",
        r#"
        <input id="before" form="f">
        <form id="f"><input id="inside"></form>
        <button id="after" form="f">Go</button>
        <script>
          const form = document.getElementById("f");
          console.log(form.elements.length);
          console.log(form.elements[0].id + "," + form.elements[1].id + "," + form.elements[2].id);
        </script>
        "#,
    );
    let browser = Browser::open(
        Box::new(loader),
        &Url::parse("demo:///form.html").unwrap(),
    )
    .unwrap();

    assert_eq!(browser.document().runtime.console.join("\n"), "3\nbefore,inside,after");
}

#[test]
fn external_control_form_property_resolves_explicit_owner() {
    let mut loader = MemoryLoader::new();
    loader.insert(
        "demo:///form.html",
        r#"
        <input id="external" form="f">
        <form id="f"></form>
        <script>console.log(document.getElementById("external").form.id);</script>
        "#,
    );
    let browser = Browser::open(
        Box::new(loader),
        &Url::parse("demo:///form.html").unwrap(),
    )
    .unwrap();

    assert_eq!(browser.document().runtime.console, vec!["f"]);
}

#[test]
fn reset_button_restores_external_controls_owned_by_form() {
    let mut browser = browser_for(
        r#"
        <input id="external" name="q" value="default" form="f">
        <form id="f" action="next">
            <button id="reset" type="reset">Reset</button>
            <button id="go">Go</button>
        </form>
        "#,
    );
    let external = dom_api::get_element_by_id(&browser.document().dom, "external").unwrap();
    let reset = dom_api::get_element_by_id(&browser.document().dom, "reset").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    let node = dom_api::node_at_mut(&mut browser.document_mut().dom, &external).unwrap();
    let NodeType::Element(element) = &mut node.node_type else {
        panic!("external input is not an element");
    };
    element.set_control_value("typed");

    let _ = browser.click_node(&reset);
    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(browser.url().to_string(), "demo:///next?q=default");
}

#[test]
fn radio_group_uses_form_owner_across_dom_locations() {
    let mut browser = browser_for(
        r#"
        <input id="outside" type="radio" name="choice" value="outside" form="f">
        <form id="f">
            <input id="inside" type="radio" name="choice" value="inside" checked>
        </form>
        "#,
    );
    let outside = dom_api::get_element_by_id(&browser.document().dom, "outside").unwrap();
    let inside = dom_api::get_element_by_id(&browser.document().dom, "inside").unwrap();

    let _ = browser.click_node(&outside);

    let outside_checked = dom_api::node_at(&browser.document().dom, &outside)
        .and_then(|node| node.as_element())
        .unwrap()
        .is_checked();
    let inside_checked = dom_api::node_at(&browser.document().dom, &inside)
        .and_then(|node| node.as_element())
        .unwrap()
        .is_checked();
    assert!(outside_checked);
    assert!(!inside_checked);
}
