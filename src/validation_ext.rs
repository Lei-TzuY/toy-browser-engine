// ============================================================
//  validation_ext.rs — path-aware validation facade
// ============================================================
//
// The core validity algorithms remain in validation.rs. This facade adds DOM
// ancestry semantics that cannot be answered from ElementData alone, notably
// disabled-fieldset inheritance and its first-legend exception.

use crate::dom::Node;
use crate::forms;
use crate::script::dom_api::NodePath;

pub use crate::validation_base::{will_validate, Validity};

/// Compute validity for a live control, including structural disabledness.
///
/// A control disabled by an ancestor `<fieldset disabled>` is barred from
/// constraint validation, except when it is inside that fieldset's first
/// `<legend>` element child. The underlying type/range/pattern algorithms stay
/// centralized in `validation.rs`.
pub fn control_validity(dom: &Node, path: &[usize]) -> Validity {
    if forms::is_effectively_disabled(dom, path) {
        return Validity::default();
    }
    crate::validation_base::control_validity(dom, path)
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
