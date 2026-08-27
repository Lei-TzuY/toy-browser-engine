use browser_engine::browser::{Browser, ClickOutcome};
use browser_engine::dom::NodeType;
use browser_engine::html::parse_html;
use browser_engine::net::Url;
use browser_engine::script::dom_api;
use browser_engine::validation;
use browser_engine::MemoryLoader;

fn path(dom: &browser_engine::dom::Node, id: &str) -> Vec<usize> {
    dom_api::get_element_by_id(dom, id).unwrap_or_else(|| panic!("missing #{id}"))
}

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

#[test]
fn required_on_one_radio_makes_its_sibling_value_missing() {
    let dom = parse_html(
        r#"<form><input id="required" type="radio" name="choice" required><input id="plain" type="radio" name="choice"></form>"#,
    );
    let required = path(&dom, "required");
    let plain = path(&dom, "plain");

    assert!(validation::control_validity(&dom, &required).value_missing);
    assert!(validation::control_validity(&dom, &plain).value_missing);
}

#[test]
fn any_checked_radio_satisfies_group_requiredness() {
    let dom = parse_html(
        r#"<form><input id="required" type="radio" name="choice" required><input id="plain" type="radio" name="choice" checked></form>"#,
    );
    let required = path(&dom, "required");
    let plain = path(&dom, "plain");

    assert!(!validation::control_validity(&dom, &required).value_missing);
    assert!(!validation::control_validity(&dom, &plain).value_missing);
}

#[test]
fn disabled_required_radio_still_requires_enabled_group_member() {
    let dom = parse_html(
        r#"<form><input id="disabled" type="radio" name="choice" required disabled><input id="enabled" type="radio" name="choice"></form>"#,
    );
    let disabled = path(&dom, "disabled");
    let enabled = path(&dom, "enabled");

    // The disabled control is barred from constraint validation itself.
    assert!(validation::control_validity(&dom, &disabled).valid());
    // Its required attribute still makes the radio group required.
    assert!(validation::control_validity(&dom, &enabled).value_missing);
}

#[test]
fn checked_disabled_radio_can_satisfy_group_requiredness() {
    let dom = parse_html(
        r#"<form><input id="disabled" type="radio" name="choice" required disabled checked><input id="enabled" type="radio" name="choice"></form>"#,
    );
    let enabled = path(&dom, "enabled");
    assert!(!validation::control_validity(&dom, &enabled).value_missing);
}

#[test]
fn equal_names_in_different_form_owners_are_not_one_group() {
    let dom = parse_html(
        r#"
        <form id="a"><input id="a-radio" type="radio" name="choice" required></form>
        <form id="b"><input id="b-radio" type="radio" name="choice" checked></form>
        "#,
    );
    let a = path(&dom, "a-radio");
    let b = path(&dom, "b-radio");

    assert!(validation::control_validity(&dom, &a).value_missing);
    assert!(!validation::control_validity(&dom, &b).value_missing);
}

#[test]
fn explicit_form_owner_groups_radios_across_dom_positions() {
    let dom = parse_html(
        r#"
        <input id="external-required" type="radio" name="choice" form="f" required>
        <form id="f"><input id="inside" type="radio" name="choice" checked></form>
        "#,
    );
    let external = path(&dom, "external-required");
    let inside = path(&dom, "inside");

    assert!(!validation::control_validity(&dom, &external).value_missing);
    assert!(!validation::control_validity(&dom, &inside).value_missing);
}

#[test]
fn unnamed_radios_do_not_share_requiredness() {
    let dom = parse_html(
        r#"<form><input id="required" type="radio" required><input id="plain" type="radio" checked></form>"#,
    );
    let required = path(&dom, "required");
    let plain = path(&dom, "plain");

    assert!(validation::control_validity(&dom, &required).value_missing);
    assert!(!validation::control_validity(&dom, &plain).value_missing);
}

#[test]
fn browser_focuses_enabled_sibling_when_disabled_radio_carries_required() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <input id="required" type="radio" name="choice" value="a" required disabled>
          <input id="enabled" type="radio" name="choice" value="b">
          <button id="go">Go</button>
        </form>
        "#,
    );
    let enabled = path(&browser.document().dom, "enabled");
    let go = path(&browser.document().dom, "go");

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&enabled));

    {
        let node = dom_api::node_at_mut(&mut browser.document_mut().dom, &enabled).unwrap();
        let NodeType::Element(element) = &mut node.node_type else {
            panic!("radio is not an element");
        };
        element.set_checked(true);
    }

    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(browser.url().to_string(), "demo:///next?choice=b");
}
