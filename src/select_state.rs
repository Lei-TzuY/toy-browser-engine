// ============================================================
//  select_state.rs  —  Live HTMLSelectElement state helpers
// ============================================================
//
//  This module is the DOM-state layer that JavaScript bindings can delegate to.
//  It deliberately owns no event dispatch: changing `value` or `selectedIndex`
//  updates selectedness, while higher layers decide whether a user action
//  should fire `input`/`change`.

use crate::dom::{ElementData, Node};
use crate::forms;
use crate::script::dom_api;

#[derive(Debug, Clone)]
struct OptionState {
    value: String,
    label: String,
    disabled: bool,
}

/// A select displayed as one collapsed control.
pub(crate) fn is_collapsed(select: &ElementData) -> bool {
    select.tag_name == "select"
        && select.get_attr("multiple").is_none()
        && select
            .get_attr("size")
            .and_then(|s| s.trim().parse::<usize>().ok())
            .unwrap_or(1)
            <= 1
}

pub(crate) fn option_labels(dom: &Node, select_path: &[usize]) -> Vec<String> {
    select_snapshot(dom, select_path)
        .unwrap_or_default()
        .into_iter()
        .map(|option| option.label)
        .collect()
}

pub(crate) fn display_label(dom: &Node, select_path: &[usize]) -> Option<String> {
    let options = select_snapshot(dom, select_path)?;
    let selected = forms::select_selected_indices(dom, select_path)?;
    Some(
        selected
            .first()
            .and_then(|index| options.get(*index))
            .map(|option| option.label.clone())
            .unwrap_or_default(),
    )
}

pub(crate) enum SelectionMove {
    Previous,
    Next,
    First,
    Last,
}

/// A user move skips disabled options/optgroups and stops at either end.
/// The caller dispatches events only when this returns true.
pub(crate) fn move_selection(
    dom: &mut Node,
    select_path: &[usize],
    movement: SelectionMove,
) -> bool {
    let Some(select) = dom_api::node_at(dom, select_path).and_then(Node::as_element) else {
        return false;
    };
    if !is_collapsed(select) || select.is_disabled() {
        return false;
    }
    let Some(options) = select_snapshot(dom, select_path) else {
        return false;
    };
    let current = forms::select_selected_indices(dom, select_path)
        .and_then(|indices| indices.first().copied());
    let enabled: Vec<_> = options
        .iter()
        .enumerate()
        .filter_map(|(index, option)| (!option.disabled).then_some(index))
        .collect();
    let next = match movement {
        SelectionMove::Next => enabled
            .iter()
            .copied()
            .find(|index| current.is_none_or(|old| *index > old)),
        SelectionMove::Previous => enabled
            .iter()
            .rev()
            .copied()
            .find(|index| current.is_none_or(|old| *index < old)),
        SelectionMove::First => enabled.first().copied(),
        SelectionMove::Last => enabled.last().copied(),
    };
    let Some(next) = next.filter(|next| Some(*next) != current) else {
        return false;
    };
    forms::set_select_selected_indices(dom, select_path, vec![next])
}

/// Current `HTMLSelectElement.value`.
///
/// For a pristine single-select with no explicit `selected` attribute, this
/// mirrors the engine's submission model and exposes the first enabled option.
/// A disabled option that is explicitly/live selected still supplies the DOM
/// value even though it will later be excluded from successful form data.
pub fn value(dom: &Node, select_path: &[usize]) -> Option<String> {
    let options = select_snapshot(dom, select_path)?;
    let selected = forms::select_selected_indices(dom, select_path)?;
    Some(
        selected
            .first()
            .and_then(|index| options.get(*index))
            .map(|option| option.value.clone())
            .unwrap_or_default(),
    )
}

/// Current `HTMLSelectElement.selectedIndex`, or `-1` when nothing is selected.
pub fn selected_index(dom: &Node, select_path: &[usize]) -> Option<isize> {
    let _ = select_snapshot(dom, select_path)?;
    Some(
        forms::select_selected_indices(dom, select_path)?
            .first()
            .copied()
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
    let Some(options) = select_snapshot(dom, select_path) else {
        return false;
    };
    let selected = options.iter().position(|option| option.value == wanted);
    forms::set_select_selected_indices(dom, select_path, selected.into_iter().collect())
}

/// Assign `HTMLSelectElement.selectedIndex`.
///
/// Negative and out-of-range indexes clear the selection; otherwise exactly
/// the option at that list index becomes selected.
pub fn set_selected_index(dom: &mut Node, select_path: &[usize], index: isize) -> bool {
    let Some(options) = select_snapshot(dom, select_path) else {
        return false;
    };
    let selected = usize::try_from(index)
        .ok()
        .filter(|index| *index < options.len());
    forms::set_select_selected_indices(dom, select_path, selected.into_iter().collect())
}

fn select_snapshot(dom: &Node, select_path: &[usize]) -> Option<Vec<OptionState>> {
    let select_node = dom_api::node_at(dom, select_path)?;
    let select = select_node.as_element()?;
    if select.tag_name != "select" {
        return None;
    }
    let mut options = Vec::new();
    collect_options(select_node, true, false, &mut options);
    Some(options)
}

fn collect_options(node: &Node, is_root: bool, disabled_group: bool, out: &mut Vec<OptionState>) {
    let mut descendants_disabled = disabled_group;
    if let Some(element) = node.as_element() {
        if !is_root && element.tag_name == "select" {
            return;
        }
        if element.tag_name == "option" {
            out.push(OptionState {
                value: option_value(node, element),
                label: element
                    .get_attr("label")
                    .map(str::to_string)
                    .unwrap_or_else(|| dom_api::text_content(node))
                    .split_ascii_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
                disabled: disabled_group || element.get_attr("disabled").is_some(),
            });
            return;
        }
        descendants_disabled |=
            element.tag_name == "optgroup" && element.get_attr("disabled").is_some();
    }
    for child in &node.children {
        collect_options(child, false, descendants_disabled, out);
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
    use crate::html::parse_html;
    use crate::script::dom_api::NodePath;

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

    #[test]
    fn generic_control_reset_restores_select_defaults() {
        let mut dom = parse_html(
            r#"<select id="pick"><option value="a" selected>A</option><option value="b">B</option></select>"#,
        );
        let select_path = select(&dom);
        assert!(set_value(&mut dom, &select_path, "b"));
        assert_eq!(value(&dom, &select_path).as_deref(), Some("b"));

        let select_element = dom_api::node_at_mut(&mut dom, &select_path)
            .and_then(|node| match &mut node.node_type {
                crate::dom::NodeType::Element(element) => Some(element),
                _ => None,
            })
            .unwrap();
        select_element.reset_control_value();

        assert_eq!(value(&dom, &select_path).as_deref(), Some("a"));
        assert_eq!(forms::select_values(&dom, &select_path), vec!["a"]);
    }
}
