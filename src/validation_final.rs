// ============================================================
//  validation_final.rs — final radio/file/color applicability overlay
// ============================================================
//
// The older validation layers intentionally stay stable because several
// stacked PRs build on them. This final wrapper repairs the remaining cases
// where those layers cannot express live, path-aware HTML semantics cleanly:
// radio groups use the canonical form-state grouping algorithm, stray readonly
// on file controls must not disable `required`, and `required` is inapplicable
// to Color state.

use crate::dom::Node;
use crate::forms;
use crate::script::dom_api::{self, NodePath};

pub use crate::validation_facade::{will_validate, Validity};

/// Compute final validity after every compatibility/facade layer has run.
pub fn control_validity(dom: &Node, path: &[usize]) -> Validity {
    let mut validity = crate::validation_facade::control_validity(dom, path);
    let Some(element) = dom_api::node_at(dom, path).and_then(|node| node.as_element()) else {
        return validity;
    };

    if !will_validate(element) || forms::is_effectively_disabled(dom, path) {
        return validity;
    }

    if element.tag_name != "input" {
        return validity;
    }

    match element.input_type().as_str() {
        // Radio requiredness is group-wide. Recompute it for every radio using
        // the canonical state helper so unowned radios and controls associated
        // through `form="…"` use the same grouping rules as checkedness writes.
        "radio" => {
            let group = crate::form_state::radio_group_paths(dom, path);
            let group_required = group.iter().any(|candidate| {
                dom_api::node_at(dom, candidate)
                    .and_then(|node| node.as_element())
                    .is_some_and(|radio| radio.get_attr("required").is_some())
            });
            let group_checked = group.iter().any(|candidate| {
                dom_api::node_at(dom, candidate)
                    .and_then(|node| node.as_element())
                    .is_some_and(|radio| radio.is_checked())
            });
            validity.value_missing = group_required && !group_checked;
        }

        // `readonly` is inapplicable to File Upload state. The lower validator
        // short-circuits on any readonly attribute, so restore Required-state
        // evaluation only for that compatibility hole.
        "file" if element.is_readonly() && element.get_attr("required").is_some() => {
            validity.value_missing = element.control_value().is_empty();
        }

        // Color state always has a value in conforming browsers and does not
        // support `required`. The engine still preserves a raw empty value, so
        // explicitly prevent that implementation detail from becoming
        // valueMissing.
        "color" => {
            validity.value_missing = false;
        }
        _ => {}
    }

    validity
}

/// Every invalid control owned by a form, in document order.
pub fn invalid_controls(dom: &Node, form_path: &[usize]) -> Vec<NodePath> {
    forms::form_controls(dom, form_path)
        .into_iter()
        .filter(|path| !control_validity(dom, path).valid())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::NodeType;
    use crate::html::parse_html;

    fn input_validity(html: &str) -> Validity {
        let dom = parse_html(html);
        let path = dom_api::query_selector(&dom, &[], "input").unwrap();
        control_validity(&dom, &path)
    }

    #[test]
    fn required_is_inapplicable_to_color_state() {
        let flags = input_validity(r#"<input type="color" required>"#);
        assert!(flags.valid());

        let flags = input_validity(r#"<input type="color" required readonly>"#);
        assert!(flags.valid());
    }

    #[test]
    fn readonly_file_still_participates_in_required_validation() {
        let mut dom = parse_html(r#"<input id="upload" type="file" required readonly>"#);
        let path = dom_api::get_element_by_id(&dom, "upload").unwrap();
        assert!(control_validity(&dom, &path).value_missing);

        if let NodeType::Element(element) = &mut dom_api::node_at_mut(&mut dom, &path).unwrap().node_type {
            element.set_control_value("picked.txt");
        }
        assert!(control_validity(&dom, &path).valid());
    }

    #[test]
    fn readonly_radio_uses_group_wide_requiredness() {
        let dom = parse_html(
            r#"<form id="f">
                <input id="a" type="radio" name="choice" required>
                <input id="b" type="radio" name="choice" readonly>
            </form>"#,
        );
        let a = dom_api::get_element_by_id(&dom, "a").unwrap();
        let b = dom_api::get_element_by_id(&dom, "b").unwrap();
        assert!(control_validity(&dom, &a).value_missing);
        assert!(control_validity(&dom, &b).value_missing);
    }

    #[test]
    fn canonical_radio_validation_handles_unowned_groups() {
        let dom = parse_html(
            r#"<input id="a" type="radio" name="choice" required>
               <input id="b" type="radio" name="choice" checked readonly>"#,
        );
        let a = dom_api::get_element_by_id(&dom, "a").unwrap();
        let b = dom_api::get_element_by_id(&dom, "b").unwrap();
        assert!(control_validity(&dom, &a).valid());
        assert!(control_validity(&dom, &b).valid());
    }

    #[test]
    fn disabled_file_and_radio_remain_barred() {
        let flags = input_validity(r#"<input type="file" required readonly disabled>"#);
        assert!(flags.valid());

        let flags = input_validity(r#"<input type="radio" required readonly disabled>"#);
        assert!(flags.valid());
    }
}
