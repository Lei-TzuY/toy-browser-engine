use browser_engine::browser::{Browser, ClickOutcome};
use browser_engine::net::Url;
use browser_engine::script::dom_api;
use browser_engine::select_state;
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
fn true_placeholder_blocks_required_single_select_submission() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <select id="pick" name="pick" required>
            <option value="">Choose one</option>
            <option value="x">X</option>
          </select>
          <button id="go">Go</button>
        </form>
        "#,
    );
    let pick = dom_api::get_element_by_id(&browser.document().dom, "pick").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&pick));
}

#[test]
fn later_empty_valued_option_is_a_real_value_for_required_select() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <select name="pick" required>
            <option value="x">X</option>
            <option value="" selected>Empty but real</option>
          </select>
          <button id="go">Go</button>
        </form>
        "#,
    );
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(browser.url().to_string(), "demo:///next?pick=");
}

#[test]
fn empty_option_inside_optgroup_is_not_a_placeholder_label_option() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <select name="pick" required>
            <optgroup label="group">
              <option value="" selected>Empty</option>
            </optgroup>
            <option value="x">X</option>
          </select>
          <button id="go">Go</button>
        </form>
        "#,
    );
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(browser.url().to_string(), "demo:///next?pick=");
}

#[test]
fn display_size_greater_than_one_disables_placeholder_semantics() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <select name="pick" required size="2">
            <option value="" selected>Empty</option>
            <option value="x">X</option>
          </select>
          <button id="go">Go</button>
        </form>
        "#,
    );
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(browser.url().to_string(), "demo:///next?pick=");
}

#[test]
fn disabled_selected_non_placeholder_option_satisfies_required_but_is_not_submitted() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <select name="pick" required>
            <option value="">Choose one</option>
            <option value="x" selected disabled>X</option>
          </select>
          <button id="go">Go</button>
        </form>
        "#,
    );
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();

    assert!(matches!(browser.click_node(&go), ClickOutcome::Navigated(_)));
    assert_eq!(browser.url().to_string(), "demo:///next");
}

#[test]
fn clearing_selected_index_makes_required_select_missing() {
    let mut browser = browser_for(
        r#"
        <form action="next">
          <select id="pick" name="pick" required>
            <option value="a">A</option>
            <option value="b">B</option>
          </select>
          <button id="go">Go</button>
        </form>
        "#,
    );
    let pick = dom_api::get_element_by_id(&browser.document().dom, "pick").unwrap();
    let go = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();
    assert!(select_state::set_selected_index(
        &mut browser.document_mut().dom,
        &pick,
        -1,
    ));
    assert_eq!(
        select_state::selected_index(&browser.document().dom, &pick),
        Some(-1)
    );

    assert_eq!(browser.click_node(&go), ClickOutcome::Script);
    assert_eq!(browser.url().to_string(), "demo:///form.html");
    assert_eq!(browser.document().focused_path().as_ref(), Some(&pick));
}
