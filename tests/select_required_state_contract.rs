use browser_engine::html::parse_html;
use browser_engine::script::dom_api;
use browser_engine::select_state;
use browser_engine::validation;

fn select_path(dom: &browser_engine::dom::Node) -> Vec<usize> {
    dom_api::query_selector(dom, &[], "select").expect("select")
}

#[test]
fn non_required_empty_placeholder_like_selection_is_not_value_missing() {
    let dom = parse_html(
        r#"<select><option value="">Choose</option><option value="x">X</option></select>"#,
    );
    let select = select_path(&dom);

    assert_eq!(select_state::required_value_missing(&dom, &select), Some(false));
    assert!(!validation::control_validity(&dom, &select).value_missing);
}

#[test]
fn clearing_a_non_required_select_stays_valid_but_required_select_is_missing() {
    let mut optional = parse_html(
        r#"<select><option value="a">A</option><option value="b">B</option></select>"#,
    );
    let optional_select = select_path(&optional);
    assert!(select_state::set_selected_index(&mut optional, &optional_select, -1));
    assert_eq!(
        select_state::required_value_missing(&optional, &optional_select),
        Some(false)
    );
    assert!(validation::control_validity(&optional, &optional_select).valid());

    let mut required = parse_html(
        r#"<select required><option value="a">A</option><option value="b">B</option></select>"#,
    );
    let required_select = select_path(&required);
    assert!(select_state::set_selected_index(&mut required, &required_select, -1));
    assert_eq!(
        select_state::required_value_missing(&required, &required_select),
        Some(true)
    );
    assert!(validation::control_validity(&required, &required_select).value_missing);
}
