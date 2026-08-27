use browser_engine::browser::{Browser, ClickOutcome};
use browser_engine::dom::NodeType;
use browser_engine::forms;
use browser_engine::html::parse_html;
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

#[test]
fn disabled_fieldset_omits_controls_except_first_legend_subtree() {
    let dom = parse_html(
        r#"
        <form id="f">
          <fieldset disabled>
            <legend><span><input id="legend" name="legend" value="keep"></span></legend>
            <input id="body" name="body" value="drop">
            <legend><input id="second" name="second" value="drop2"></legend>
          </fieldset>
        </form>
        "#,
    );
    let form = dom_api::get_element_by_id(&dom, "f").unwrap();
    let legend = dom_api::get_element_by_id(&dom, "legend").unwrap();
    let body = dom_api::get_element_by_id(&dom, "body").unwrap();
    let second = dom_api::get_element_by_id(&dom, "second").unwrap();

    assert!(!forms::is_effectively_disabled(&dom, &legend));
    assert!(forms::is_effectively_disabled(&dom, &body));
    assert!(forms::is_effectively_disabled(&dom, &second));
    assert_eq!(
        forms::form_data(&dom, &form),
        vec![("legend".into(), "keep".into())]
    );
}

#[test]
fn nested_disabled_fieldsets_apply_each_legend_exception_independently() {
    let dom = parse_html(
        r#"
        <form id="f">
          <fieldset disabled>
            <legend>
              <fieldset disabled>
                <legend><input id="double-exempt" name="a" value="1"></legend>
                <input id="inner-body" name="b" value="2">
              </fieldset>
            </legend>
          </fieldset>
        </form>
        "#,
    );
    let form = dom_api::get_element_by_id(&dom, "f").unwrap();
    let exempt = dom_api::get_element_by_id(&dom, "double-exempt").unwrap();
    let inner_body = dom_api::get_element_by_id(&dom, "inner-body").unwrap();

    assert!(!forms::is_effectively_disabled(&dom, &exempt));
    assert!(forms::is_effectively_disabled(&dom, &inner_body));
    assert_eq!(forms::form_data(&dom, &form), vec![("a".into(), "1".into())]);
}

#[test]
fn form_attribute_does_not_escape_structural_fieldset_disabledness() {
    let dom = parse_html(
        r#"
        <form id="target"></form>
        <fieldset disabled>
          <input id="external" name="q" value="x" form="target">
        </fieldset>
        "#,
    );
    let target = dom_api::get_element_by_id(&dom, "target").unwrap();
    let external = dom_api::get_element_by_id(&dom, "external").unwrap();

    assert_eq!(forms::owning_form(&dom, &external), Some(target.clone()));
    assert!(forms::is_effectively_disabled(&dom, &external));
    assert!(forms::form_data(&dom, &target).is_empty());
}

#[test]
fn inherited_disabled_required_control_does_not_block_browser_submission() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <fieldset disabled>
            <input id="blocked" name="blocked" required>
          </fieldset>
          <input name="ok" value="yes">
          <button id="go">Go</button>
        </form>
        "#,
    );
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(browser.url().to_string(), "demo:///next?ok=yes");
}

#[test]
fn required_control_in_first_legend_still_blocks_and_focuses() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <fieldset disabled>
            <legend><input id="legend" name="legend" required></legend>
            <input id="blocked" name="blocked" required>
          </fieldset>
          <button id="go">Go</button>
        </form>
        "#,
    );
    let legend = dom_api::get_element_by_id(&browser.document().dom, "legend").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&legend));
}

#[test]
fn removing_fieldset_disabled_reenables_validation_and_serialization_live() {
    let mut browser = browser_for(
        r#"
        <form id="f" action="next">
          <fieldset id="group" disabled>
            <input id="q" name="q" required>
          </fieldset>
          <button id="go">Go</button>
        </form>
        "#,
    );
    let form = dom_api::get_element_by_id(&browser.document().dom, "f").unwrap();
    let group = dom_api::get_element_by_id(&browser.document().dom, "group").unwrap();
    let q = dom_api::get_element_by_id(&browser.document().dom, "q").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert!(forms::is_effectively_disabled(&browser.document().dom, &q));
    assert!(forms::form_data(&browser.document().dom, &form).is_empty());

    {
        let node = dom_api::node_at_mut(&mut browser.document_mut().dom, &group).unwrap();
        let NodeType::Element(element) = &mut node.node_type else {
            panic!("fieldset node is not an element");
        };
        element.remove_attr("disabled");
    }

    assert!(!forms::is_effectively_disabled(&browser.document().dom, &q));
    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.document().focused_path().as_ref(), Some(&q));

    {
        let node = dom_api::node_at_mut(&mut browser.document_mut().dom, &q).unwrap();
        let NodeType::Element(element) = &mut node.node_type else {
            panic!("input node is not an element");
        };
        element.set_control_value("live");
    }
    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(browser.url().to_string(), "demo:///next?q=live");
}

#[test]
fn tab_order_skips_fieldset_disabled_controls_but_keeps_first_legend() {
    let dom = parse_html(
        r#"
        <fieldset disabled>
          <legend><input id="legend"></legend>
          <input id="blocked">
        </fieldset>
        <button id="after">After</button>
        "#,
    );
    let ids: Vec<String> = forms::tab_order(&dom)
        .into_iter()
        .filter_map(|path| {
            dom_api::node_at(&dom, &path)
                .and_then(|node| node.as_element())
                .and_then(|element| element.get_attr("id"))
                .map(str::to_string)
        })
        .collect();
    assert_eq!(ids, vec!["legend", "after"]);
}
