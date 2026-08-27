// ============================================================
//  validation_facade.rs — final path-aware validation facade
// ============================================================
//
// Most validation lives in validation_constraints. This final facade overlays
// select-specific required/placeholder semantics that depend on the option tree
// and selectedness rather than only on the select's submitted string value.

use crate::dom::Node;
use crate::forms;
use crate::script::dom_api::{self, NodePath};

pub use crate::validation_constraints::{will_validate, Validity};

/// Compute the final validity state for a live form control.
pub fn control_validity(dom: &Node, path: &[usize]) -> Validity {
    let mut validity = crate::validation_constraints::control_validity(dom, path);
    let Some(element) = dom_api::node_at(dom, path).and_then(|node| node.as_element()) else {
        return validity;
    };

    if element.tag_name == "select"
        && element.get_attr("required").is_some()
        && will_validate(element)
        && !forms::is_effectively_disabled(dom, path)
    {
        validity.value_missing = crate::select_state::required_value_missing(dom, path)
            .unwrap_or(validity.value_missing);
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
    use crate::html::parse_html;

    fn select_validity(html: &str) -> Validity {
        let dom = parse_html(html);
        let path = dom_api::query_selector(&dom, &[], "select").unwrap();
        control_validity(&dom, &path)
    }

    #[test]
    fn required_select_rejects_only_the_true_placeholder_label_option() {
        let flags = select_validity(
            r#"<select required><option value="">Choose</option><option value="x">X</option></select>"#,
        );
        assert!(flags.value_missing);

        let flags = select_validity(
            r#"<select required><option value="x">X</option><option value="" selected>Empty</option></select>"#,
        );
        assert!(!flags.value_missing);
    }

    #[test]
    fn effective_disabledness_still_bars_required_select_validation() {
        let flags = select_validity(
            r#"<fieldset disabled><select required><option value="">Choose</option></select></fieldset>"#,
        );
        assert!(flags.valid());
    }
}
