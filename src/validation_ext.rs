// ============================================================
//  validation_ext.rs — path-aware validation facade
// ============================================================
//
// The core validity algorithms remain in validation.rs. This facade adds DOM
// relationships that cannot be answered from ElementData alone: inherited
// fieldset disabledness and radio-button group-wide requiredness.

use crate::dom::Node;
use crate::forms;
use crate::script::dom_api::{self, NodePath};

pub use crate::validation_base::{will_validate, Validity};

/// Compute validity for a live control, including structural/group semantics.
///
/// A control disabled by an ancestor `<fieldset disabled>` is barred from
/// constraint validation, except when it is inside that fieldset's first
/// `<legend>` element child. Radio `required` is group-wide: if any radio in a
/// same-name/same-form-owner group is required, every validating member suffers
/// from `valueMissing` until one member is checked.
pub fn control_validity(dom: &Node, path: &[usize]) -> Validity {
    if forms::is_effectively_disabled(dom, path) {
        return Validity::default();
    }

    let mut validity = crate::validation_base::control_validity(dom, path);
    let Some(element) = dom_api::node_at(dom, path).and_then(|node| node.as_element()) else {
        return validity;
    };
    if element.tag_name == "input" && element.input_type() == "radio" {
        let group = crate::form_state::radio_group_paths(dom, path);
        let required = group.iter().any(|candidate| {
            dom_api::node_at(dom, candidate)
                .and_then(|node| node.as_element())
                .is_some_and(|radio| radio.get_attr("required").is_some())
        });
        let checked = group.iter().any(|candidate| {
            dom_api::node_at(dom, candidate)
                .and_then(|node| node.as_element())
                .is_some_and(|radio| radio.is_checked())
        });
        validity.value_missing = required && !checked;
    }
    validity
}

/// Every invalid control owned by a form, in document order.
///
/// `form_controls()` deliberately includes disabled controls because they still
/// belong to `form.elements` and participate in reset. Constraint validation
/// filters them here instead.
pub fn invalid_controls(dom: &Node, form_path: &[usize]) -> Vec<NodePath> {
    forms::form_controls(dom, form_path)
        .into_iter()
        .filter(|path| !control_validity(dom, path).valid())
        .collect()
}
