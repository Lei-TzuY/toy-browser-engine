// ============================================================
//  select_state.rs  —  Live HTMLSelectElement state helpers
// ============================================================
//
//  This module is the DOM-state layer that JavaScript bindings can delegate to.
//  It deliberately owns no event dispatch: changing `value` or `selectedIndex`
//  updates option selectedness, while higher layers decide whether a user action
//  should fire `input`/`change`.

use crate::dom::{ElementData, Node, NodeType};
use crate::script::dom_api::{self, NodePath};

#[derive(Debug, Clone)]
struct OptionState {
    path: NodePath,
    selected: bool,
    selected_is_default: bool,
    disabled: bool,
    value: String,
}

/// Current `HTMLSelectElement.value`.
///
/// For a pristine single-select with no explicit `selected` attribute, this
/// mirrors the engine's submission model and exposes the first enabled option.
/// A disabled option that is explicitly/live selected still supplies the DOM
/// value even though it will later be excluded from successful form data.
pub fn value(dom: &Node, select_path: &[usize]) -> Option<String> {
    let (multiple, options) = select_snapshot(dom, select_path)?;
    effective_selected_index(multiple, &options)
        .map(|index| options[index].value.clone())
        .or_else(|| Some(String::new()))
}

/// Current `HTMLSelectElement.selectedIndex`, or `-1` when nothing is selected.
pub fn selected_index(dom: &Node, select_path: &[usize]) -> Option<isize> {
    let (multiple, options) = select_snapshot(dom, select_path)?;
    Some(
        effective_selected_index(multiple, &options)
            .map(|index| index as isize)
            .unwrap_or(-1),
    )
}

/// Assign `HTMLSelectElement.value`.
///
/// The first option whose submission value exactly equals `wanted` becomes the
/// only selected option. If none matches, every option becomes unselected.
/// This applies to `multiple` selects too, matching the DOM property setter.
pub fn set_value(dom: &mut Node, select_path: &[usize], wanted: &str) -> bool {
    let Some((_, options)) = select_snapshot(dom, select_path) else {
        return false;
    };
    let selected = options.iter().position(|option| option.value == wanted);
    apply_single_selection(dom, &options, selected);
    true
}

/// Assign `HTMLSelectElement.selectedIndex`.
///
/// Negative and out-of-range indexes clear the selection; otherwise exactly
/// the option at that list index becomes selected.
pub fn set_selected_index(dom: &mut Node, select_path: &[usize], index: isize) -> bool {
    let Some((_, options)) = select_snapshot(dom, select_path) else {
        return false;
    };
    let selected = usize::try_from(index)
        .ok()
        .filter(|index| *index < options.len());
    apply_single_selection(dom, &options, selected);
    true
}

fn apply_single_selection(dom: &mut Node, options: &[OptionState], selected: Option<usize>) {
    for (index, option) in options.iter().enumerate() {
        let Some(node) = dom_api::node_at_mut(dom, &option.path) else {
            continue;
        };
        let NodeType::Element(element) = &mut node.node_type else {
            continue;
        };
        element.set_selected(selected == Some(index));
    }
}

fn effective_selected_index(multiple: bool, options: &[OptionState]) -> Option<usize> {
    if multiple {
        return options.iter().position(|option| option.selected);
    }
    if let Some(index) = options.iter().rposition(|option| option.selected) {
        return Some(index);
    }
    if options.iter().any(|option| !option.selected_is_default) {
        return None;
    }
    options.iter().position(|option| !option.disabled)
}

fn select_snapshot(dom: &Node, select_path: &[usize]) -> Option<(bool, Vec<OptionState>)> {
    let select_node = dom_api::node_at(dom, select_path)?;
    let select = select_node.as_element()?;
    if select.tag_name != "select" {
        return None;
    }
    let multiple = select.get_attr("multiple").is_some();
    let mut options = Vec::new();
    collect_options(
        select_node,
        &mut select_path.to_vec(),
        false,
        true,
        &mut options,
    );
    Some((multiple, options))
}

fn collect_options(
    node: &Node,
    path: &mut NodePath,
    disabled_group: bool,
    is_root: bool,
    out: &mut Vec<OptionState>,
) {
    let mut descendants_disabled = disabled_group;
    if let Some(element) = node.as_element() {
        if !is_root && element.tag_name == "select" {
            return;
        }
        if element.tag_name == "optgroup" && element.get_attr("disabled").is_some() {
            descendants_disabled = true;
        }
        if element.tag_name == "option" {
            out.push(OptionState {
                path: path.clone(),
                selected: element.is_selected(),
                selected_is_default: element.selected_is_default(),
                disabled: descendants_disabled || element.get_attr("disabled").is_some(),
                value: option_value(node, element),
            });
            return;
        }
    }
    for (index, child) in node.children.iter().enumerate() {
        path.push(index);
        collect_options(child, path, descendants_disabled, false, out);
        path.pop();
    }
}

fn option_value(node: &Node, element: &ElementData) -> String {
    element
        .get_attr("value")
        .map(str::to_string)
        .unwrap_or_else(|| {
            dom_api::text_content(node)
                .split_ascii_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forms;
    use crate::html::parse_html;

    fn select(dom: &Node) -> NodePath {
        dom_api::query_selector(dom, &[], "select").expect("select")
    }

    #[test]
    fn pristine_single_select_exposes_the_effective_first_option() {
        let dom = parse_html(
            r#"<select><option disabled value="x">X</option><option value="a">A</option></select>"#,
        );
        let select = select(&dom);
        assert_eq!(value(&dom, &select).as_deref(), Some("a"));
        assert_eq!(selected_index(&dom, &select), Some(1));
    }

    #[test]
    fn single_select_uses_the_last_explicit_selected_option() {
        let dom = parse_html(
            r#"<select><option selected value="a">A</option><option selected value="b">B</option></select>"#,
        );
        let select = select(&dom);
        assert_eq!(value(&dom, &select).as_deref(), Some("b"));
        assert_eq!(selected_index(&dom, &select), Some(1));
    }

    #[test]
    fn selected_disabled_option_is_visible_to_dom_but_not_form_data() {
        let dom = parse_html(
            r#"<form><select name="pick"><option selected disabled value="x">X</option><option value="a">A</option></select></form>"#,
        );
        let select = select(&dom);
        let form = dom_api::query_selector(&dom, &[], "form").unwrap();
        assert_eq!(value(&dom, &select).as_deref(), Some("x"));
        assert!(forms::form_data(&dom, &form).is_empty());
    }

    #[test]
    fn value_setter_selects_first_match_and_clears_others() {
        let mut dom = parse_html(
            r#"<select multiple><option selected value="a">A</option><option selected value="b">B</option><option value="b">B2</option></select>"#,
        );
        let select = select(&dom);
        assert!(set_value(&mut dom, &select, "b"));
        assert_eq!(value(&dom, &select).as_deref(), Some("b"));
        assert_eq!(selected_index(&dom, &select), Some(1));
        assert_eq!(forms::select_values(&dom, &select), vec!["b"]);
    }

    #[test]
    fn unknown_value_clears_selection_without_reapplying_fallback() {
        let mut dom = parse_html(
            r#"<select><option value="a">A</option><option value="b">B</option></select>"#,
        );
        let select = select(&dom);
        assert_eq!(value(&dom, &select).as_deref(), Some("a"));
        assert!(set_value(&mut dom, &select, "missing"));
        assert_eq!(value(&dom, &select).as_deref(), Some(""));
        assert_eq!(selected_index(&dom, &select), Some(-1));
        assert!(forms::select_values(&dom, &select).is_empty());
    }

    #[test]
    fn selected_index_setter_selects_or_clears_by_list_position() {
        let mut dom = parse_html(
            r#"<select><option value="a">A</option><option disabled value="b">B</option><option value="c">C</option></select>"#,
        );
        let select = select(&dom);
        assert!(set_selected_index(&mut dom, &select, 1));
        assert_eq!(value(&dom, &select).as_deref(), Some("b"));
        assert_eq!(selected_index(&dom, &select), Some(1));
        assert!(set_selected_index(&mut dom, &select, 99));
        assert_eq!(selected_index(&dom, &select), Some(-1));
    }
}
