// ============================================================
//  validation_facade.rs — final path-aware validation facade
// ============================================================
//
// Most validation lives in validation_constraints. This final facade overlays
// structural and lexical rules that need stricter live-DOM information than the
// older element-only validators expose: select placeholder selectedness and the
// Number state's exact valid-floating-point-number syntax.

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

    let participates = will_validate(element) && !forms::is_effectively_disabled(dom, path);

    if element.tag_name == "select"
        && element.get_attr("required").is_some()
        && participates
    {
        validity.value_missing = crate::select_state::required_value_missing(dom, path)
            .unwrap_or(validity.value_missing);
    }

    // The base number validator deliberately keeps a raw live string, but its
    // numeric parser trims whitespace and delegates to Rust's permissive float
    // parser. HTML's Number state is stricter: the value must itself be a valid
    // floating-point-number string. A real browser would sanitize invalid
    // strings back to empty; this engine preserves them while editing, so the
    // equivalent observable state is `bad_input`.
    if participates && element.tag_name == "input" && element.input_type() == "number" {
        let raw = element.control_value();
        if !raw.is_empty() && !valid_number_state_syntax(&raw) {
            validity.bad_input = true;
            // Range and step constraints only apply after the value parses as a
            // Number-state value; do not leak flags computed from a trimmed or
            // otherwise more permissive host-language parse.
            validity.range_underflow = false;
            validity.range_overflow = false;
            validity.step_mismatch = false;
        }
    }

    validity
}

/// HTML's valid floating-point-number lexical grammar used by `type=number`.
///
/// Accepted examples include `12`, `-0.5`, `.5`, `-.5`, `1e2`, and `1e+2`.
/// A leading `+`, surrounding whitespace, a trailing decimal point, a missing
/// exponent, non-ASCII digits, and trailing junk are all invalid.
fn valid_number_state_syntax(value: &str) -> bool {
    if value.is_empty() || !value.is_ascii() {
        return false;
    }

    let bytes = value.as_bytes();
    let mut index = 0usize;

    if bytes.get(index) == Some(&b'-') {
        index += 1;
    }

    let integer_start = index;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
    }
    let integer_digits = index - integer_start;

    let mut fraction_digits = 0usize;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let fraction_start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        fraction_digits = index - fraction_start;
        // If a decimal point is present, at least one digit must follow it.
        if fraction_digits == 0 {
            return false;
        }
    }

    if integer_digits == 0 && fraction_digits == 0 {
        return false;
    }

    if matches!(bytes.get(index), Some(b'e' | b'E')) {
        index += 1;
        if matches!(bytes.get(index), Some(b'+' | b'-')) {
            index += 1;
        }
        let exponent_start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if index == exponent_start {
            return false;
        }
    }

    index == bytes.len()
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

    fn select_validity(html: &str) -> Validity {
        let dom = parse_html(html);
        let path = dom_api::query_selector(&dom, &[], "select").unwrap();
        control_validity(&dom, &path)
    }

    fn number_validity(value: &str) -> Validity {
        let dom = parse_html(&format!(r#"<input type="number" value="{value}">"#));
        let path = dom_api::query_selector(&dom, &[], "input").unwrap();
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

    #[test]
    fn number_state_rejects_host_float_syntax_that_html_does_not_allow() {
        for value in [" 12 ", "+12", "12.", "1.e2", "1e", "1e+", "12x", "１２"] {
            let flags = number_validity(value);
            assert!(flags.bad_input, "{value:?} must be bad input: {flags:?}");
        }
    }

    #[test]
    fn number_state_accepts_html_float_syntax_variants() {
        for value in ["12", "-12", "0.5", ".5", "-.5", "1e2", "1E-2", "1e+2"] {
            let flags = number_validity(value);
            assert!(!flags.bad_input, "{value:?} must parse: {flags:?}");
        }
    }

    #[test]
    fn malformed_number_does_not_keep_range_or_step_flags_from_trimmed_parse() {
        let mut dom = parse_html(r#"<input id="n" type="number" min="20" step="3">"#);
        let path = dom_api::get_element_by_id(&dom, "n").unwrap();
        if let NodeType::Element(element) = &mut dom_api::node_at_mut(&mut dom, &path).unwrap().node_type {
            element.set_control_value(" 12 ");
        }

        let flags = control_validity(&dom, &path);
        assert!(flags.bad_input);
        assert!(!flags.range_underflow);
        assert!(!flags.range_overflow);
        assert!(!flags.step_mismatch);
    }

    #[test]
    fn effectively_disabled_malformed_number_remains_barred_from_validation() {
        let dom = parse_html(r#"<fieldset disabled><input id="n" type="number" value="+12"></fieldset>"#);
        let path = dom_api::get_element_by_id(&dom, "n").unwrap();
        assert!(control_validity(&dom, &path).valid());
    }
}
