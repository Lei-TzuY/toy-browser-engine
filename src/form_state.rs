// ============================================================
//  form_state.rs  —  Shared form-control state algorithms
// ============================================================
//
//  Event dispatch belongs to Document/Browser. This module only mutates the
//  underlying live control state so button activation, `form.reset()`, and DOM
//  bindings can share one state algorithm without duplicating per-control rules.

use crate::dom::{Node, NodeType};
use crate::forms;
use crate::script::dom_api::{self, NodePath};

/// Members of the radio-button group containing `path`, in document order.
///
/// HTML groups radios by radio type, the same tree, equal non-empty `name`, and
/// the same form owner. `None` is a real owner state here: two unowned radios
/// with the same non-empty name belong to one group. An unnamed radio is a
/// one-element group.
pub fn radio_group_paths(dom: &Node, path: &[usize]) -> Vec<NodePath> {
    let Some(element) = dom_api::node_at(dom, path).and_then(|node| node.as_element()) else {
        return Vec::new();
    };
    if element.tag_name != "input" || element.input_type() != "radio" {
        return Vec::new();
    }
    let Some(name) = element.get_attr("name").filter(|name| !name.is_empty()) else {
        return vec![path.to_vec()];
    };
    let owner = forms::owning_form(dom, path);

    let mut out = Vec::new();
    collect_radio_group(dom, &mut Vec::new(), name, owner.as_deref(), dom, &mut out);
    out
}

fn collect_radio_group(
    node: &Node,
    path: &mut NodePath,
    name: &str,
    owner: Option<&[usize]>,
    dom: &Node,
    out: &mut Vec<NodePath>,
) {
    if let Some(element) = node.as_element() {
        let same_radio = element.tag_name == "input"
            && element.input_type() == "radio"
            && element.get_attr("name") == Some(name);
        if same_radio && forms::owning_form(dom, path).as_deref() == owner {
            out.push(path.clone());
        }
    }
    for (index, child) in node.children.iter().enumerate() {
        path.push(index);
        collect_radio_group(child, path, name, owner, dom, out);
        path.pop();
    }
}

/// Assign current checkedness while preserving radio-group exclusivity.
///
/// Setting a radio to true silently clears every other member of the same
/// group, including disabled members. Setting a radio to false affects only
/// that radio. Checkboxes are assigned directly. Returns whether the target's
/// own checkedness changed.
pub fn set_checked(dom: &mut Node, path: &[usize], checked: bool) -> bool {
    let Some(element) = dom_api::node_at(dom, path).and_then(|node| node.as_element()) else {
        return false;
    };
    if !element.is_checkable() {
        return false;
    }
    let before = element.is_checked();
    let is_radio = element.input_type() == "radio";
    let group = if is_radio && checked {
        radio_group_paths(dom, path)
    } else {
        Vec::new()
    };

    if is_radio && checked {
        for candidate in group {
            if candidate == path {
                continue;
            }
            if let Some(NodeType::Element(other)) =
                dom_api::node_at_mut(dom, &candidate).map(|node| &mut node.node_type)
            {
                other.set_checked(false);
            }
        }
    }

    if let Some(NodeType::Element(element)) =
        dom_api::node_at_mut(dom, path).map(|node| &mut node.node_type)
    {
        element.set_checked(checked);
    }
    before != checked
}

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

    fn checked(dom: &Node, id: &str) -> bool {
        let path = path(dom, id);
        dom_api::node_at(dom, &path)
            .unwrap()
            .as_element()
            .unwrap()
            .is_checked()
    }

    #[test]
    fn setting_unowned_radio_checked_clears_its_same_name_peer() {
        let mut dom = parse_html(
            r#"<input id="a" type="radio" name="choice" checked>
               <div><input id="b" type="radio" name="choice"></div>"#,
        );
        let b = path(&dom, "b");
        assert!(set_checked(&mut dom, &b, true));
        assert!(!checked(&dom, "a"));
        assert!(checked(&dom, "b"));
    }

    #[test]
    fn radio_groups_are_isolated_by_form_owner() {
        let mut dom = parse_html(
            r#"<form id="a"><input id="a1" type="radio" name="choice" checked></form>
               <form id="b"><input id="b1" type="radio" name="choice"></form>"#,
        );
        let b1 = path(&dom, "b1");
        assert!(set_checked(&mut dom, &b1, true));
        assert!(checked(&dom, "a1"));
        assert!(checked(&dom, "b1"));
    }

    #[test]
    fn explicit_form_owner_groups_radios_across_dom_positions() {
        let mut dom = parse_html(
            r#"<input id="outside" type="radio" name="choice" form="f" checked>
               <form id="f"><input id="inside" type="radio" name="choice"></form>"#,
        );
        let inside = path(&dom, "inside");
        assert!(set_checked(&mut dom, &inside, true));
        assert!(!checked(&dom, "outside"));
        assert!(checked(&dom, "inside"));
    }

    #[test]
    fn checked_disabled_radio_is_cleared_when_a_peer_becomes_checked() {
        let mut dom = parse_html(
            r#"<form><input id="disabled" type="radio" name="choice" checked disabled>
               <input id="enabled" type="radio" name="choice"></form>"#,
        );
        let enabled = path(&dom, "enabled");
        assert!(set_checked(&mut dom, &enabled, true));
        assert!(!checked(&dom, "disabled"));
        assert!(checked(&dom, "enabled"));
    }

    #[test]
    fn setting_radio_false_does_not_select_or_clear_any_peer() {
        let mut dom = parse_html(
            r#"<input id="a" type="radio" name="choice" checked>
               <input id="b" type="radio" name="choice">"#,
        );
        let b = path(&dom, "b");
        assert!(!set_checked(&mut dom, &b, false));
        assert!(checked(&dom, "a"));
        assert!(!checked(&dom, "b"));
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
        let box_element = dom_api::node_at(&dom, &box_path).unwrap().as_element().unwrap();
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
