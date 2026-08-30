// ============================================================
//  form_state.rs  —  Shared form-control state algorithms
// ============================================================
//
//  Event dispatch belongs to Document/Browser. This module only mutates the
//  underlying live control state so button activation, `form.reset()`, and
//  future DOM bindings can share one reset algorithm without duplicating the
//  per-control rules.

use crate::dom::{Node, NodeType};
use crate::forms;
use crate::script::dom_api;

/// Restore every control associated with a form to its HTML/default state.
///
/// Text-like controls drop their live value, checkboxes/radios drop live
/// checkedness, and `<select>` controls restore each option's live selectedness
/// to the `selected` content attribute. The operation is intentionally silent:
/// programmatic form reset does not synthesize `input` or `change` events.
pub fn reset_form_state(dom: &mut Node, form_path: &[usize]) -> bool {
    let is_form = dom_api::node_at(dom, form_path)
        .and_then(|node| node.as_element())
        .is_some_and(|element| element.tag_name == "form");
    if !is_form {
        return false;
    }

    // Snapshot paths before mutating. Current reset operations do not change
    // tree shape, but this also keeps the mutable borrow local to each control.
    let controls = forms::form_controls(dom, form_path);
    for path in controls {
        let is_select = dom_api::node_at(dom, &path)
            .and_then(|node| node.as_element())
            .is_some_and(|element| element.tag_name == "select");
        if is_select {
            forms::reset_select_selectedness(dom, &path);
            continue;
        }

        if let Some(node) = dom_api::node_at_mut(dom, &path) {
            if let NodeType::Element(element) = &mut node.node_type {
                element.reset_control_value();
                element.reset_checked();
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::parse_html;

    fn path(dom: &Node, id: &str) -> Vec<usize> {
        dom_api::get_element_by_id(dom, id).expect("element")
    }

    #[test]
    fn reset_restores_text_checkable_and_select_defaults_together() {
        let mut dom = parse_html(
            r#"<form id="f">
                <input id="text" name="q" value="default">
                <input id="box" type="checkbox" checked>
                <input id="r1" type="radio" name="r" checked>
                <input id="r2" type="radio" name="r">
                <select id="pick" name="pick">
                    <option id="a" value="a" selected>A</option>
                    <option id="b" value="b">B</option>
                </select>
            </form>"#,
        );
        let form = path(&dom, "f");
        let text = path(&dom, "text");
        let box_path = path(&dom, "box");
        let r1 = path(&dom, "r1");
        let r2 = path(&dom, "r2");
        let pick = path(&dom, "pick");
        let b = path(&dom, "b");

        if let NodeType::Element(element) =
            &mut dom_api::node_at_mut(&mut dom, &text).unwrap().node_type
        {
            element.set_control_value("edited");
        }
        if let NodeType::Element(element) =
            &mut dom_api::node_at_mut(&mut dom, &box_path).unwrap().node_type
        {
            element.set_checked(false);
        }
        if let NodeType::Element(element) =
            &mut dom_api::node_at_mut(&mut dom, &r1).unwrap().node_type
        {
            element.set_checked(false);
        }
        if let NodeType::Element(element) =
            &mut dom_api::node_at_mut(&mut dom, &r2).unwrap().node_type
        {
            element.set_checked(true);
        }
        assert!(forms::set_option_selected(&mut dom, &b, true));
        assert_eq!(forms::select_values(&dom, &pick), vec!["b"]);

        assert!(reset_form_state(&mut dom, &form));

        let text_element = dom_api::node_at(&dom, &text).unwrap().as_element().unwrap();
        let box_element = dom_api::node_at(&dom, &box_path)
            .unwrap()
            .as_element()
            .unwrap();
        let r1_element = dom_api::node_at(&dom, &r1).unwrap().as_element().unwrap();
        let r2_element = dom_api::node_at(&dom, &r2).unwrap().as_element().unwrap();
        assert_eq!(text_element.control_value(), "default");
        assert!(box_element.is_checked());
        assert!(r1_element.is_checked());
        assert!(!r2_element.is_checked());
        assert_eq!(forms::select_values(&dom, &pick), vec!["a"]);
    }

    #[test]
    fn reset_reenables_pristine_single_select_fallback() {
        let mut dom = parse_html(
            r#"<form id="f"><select id="pick"><option id="a" value="a">A</option><option value="b">B</option></select></form>"#,
        );
        let form = path(&dom, "f");
        let pick = path(&dom, "pick");
        let a = path(&dom, "a");
        assert_eq!(forms::select_values(&dom, &pick), vec!["a"]);
        assert!(forms::set_option_selected(&mut dom, &a, false));
        assert!(forms::select_values(&dom, &pick).is_empty());

        assert!(reset_form_state(&mut dom, &form));
        assert_eq!(forms::select_values(&dom, &pick), vec!["a"]);
    }

    #[test]
    fn reset_refuses_a_non_form_path() {
        let mut dom = parse_html(r#"<div id="x"><input value="a"></div>"#);
        let div = path(&dom, "x");
        assert!(!reset_form_state(&mut dom, &div));
    }
}
