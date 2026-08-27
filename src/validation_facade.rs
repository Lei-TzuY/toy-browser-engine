// ============================================================
//  validation_facade.rs — final path-aware validation facade
// ============================================================
//
// Most validation lives in validation_constraints. This final facade overlays
// structural and lexical rules that need stricter live-DOM information than the
// older element-only validators expose: select placeholder selectedness, exact
// Number-state syntax, and per-input-state attribute applicability.

use crate::dom::{ElementData, Node};
use crate::forms;
use crate::script::dom_api::{self, NodePath};

pub use crate::validation_constraints::Validity;

/// Whether a control participates in constraint validation.
///
/// `readonly` is not a universal form-control switch. HTML only makes it bar
/// validation for text/date/time/number-like input states and `<textarea>`.
/// On checkbox, radio, file, range, color and `<select>` it is inapplicable and
/// must not silently disable `required` or the rest of constraint validation.
pub fn will_validate(element: &ElementData) -> bool {
    if !element.is_form_control() || element.is_disabled() {
        return false;
    }
    match element.tag_name.as_str() {
        "button" => false,
        "textarea" => !element.is_readonly(),
        "input" => {
            let kind = element.input_type();
            if matches!(kind.as_str(), "hidden" | "submit" | "reset" | "button" | "image") {
                return false;
            }
            !(element.is_readonly() && readonly_applies_to_input(&kind))
        }
        _ => true,
    }
}

fn readonly_applies_to_input(kind: &str) -> bool {
    matches!(
        kind,
        "text"
            | "search"
            | "tel"
            | "url"
            | "email"
            | "password"
            | "date"
            | "month"
            | "week"
            | "time"
            | "datetime-local"
            | "number"
    )
}

/// Compute the final validity state for a live form control.
pub fn control_validity(dom: &Node, path: &[usize]) -> Validity {
    let mut validity = crate::validation_constraints::control_validity(dom, path);
    let Some(element) = dom_api::node_at(dom, path).and_then(|node| node.as_element()) else {
        return validity;
    };

    let participates = will_validate(element) && !forms::is_effectively_disabled(dom, path);
    if !participates {
        return Validity::default();
    }

    let input_type = element.input_type();
    let required = element.get_attr("required").is_some();

    // The lower validation layers historically treated every `readonly`
    // attribute as barring validation. When readonly is inapplicable, repair
    // the required-state flags that those layers therefore skipped entirely.
    // Other currently implemented constraints do not apply to these states.
    let lower_barred_only_by_readonly = element.is_readonly()
        && crate::validation_constraints::will_validate(element) == false
        && match element.tag_name.as_str() {
            "select" => true,
            "input" => !readonly_applies_to_input(&input_type),
            _ => false,
        };

    if lower_barred_only_by_readonly && element.tag_name == "input" && required {
        validity.value_missing = match input_type.as_str() {
            "checkbox" => !element.is_checked(),
            "radio" => {
                let group = crate::form_state::radio_group_paths(dom, path);
                let group_required = group.iter().any(|candidate| {
                    dom_api::node_at(dom, candidate)
                        .and_then(|node| node.as_element())
                        .is_some_and(|radio| radio.get_attr("required").is_some())
                });
                let checked = group.iter().any(|candidate| {
                    dom_api::node_at(dom, candidate)
                        .and_then(|node| node.as_element())
                        .is_some_and(|radio| radio.is_checked())
                });
                group_required && !checked
            }
            "file" => element.control_value().is_empty(),
            _ => false,
        };
    }

    if element.tag_name == "select" && required {
        validity.value_missing = crate::select_state::required_value_missing(dom, path)
            .unwrap_or(validity.value_missing);
    }

    // `required` is not applicable to the Range or Color states. They always
    // have a value in conforming browsers, so an author-supplied `required`
    // attribute must never make them value-missing even in this engine's raw
    // value model.
    if element.tag_name == "input" && matches!(input_type.as_str(), "range" | "color") {
        validity.value_missing = false;
    }

    // The base number validator deliberately keeps a raw live string, but its
    // numeric parser trims whitespace and delegates to Rust's permissive float
    // parser. HTML's Number state is stricter: the value must itself be a valid
    // floating-point-number string. A real browser would sanitize invalid
    // strings back to empty; this engine preserves them while editing, so the
    // equivalent observable state is `bad_input`.
    if element.tag_name == "input" && input_type == "number" {
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

    fn input_validity(html: &str) -> Validity {
        let dom = parse_html(html);
        let path = dom_api::query_selector(&dom, &[], "input").unwrap();
        control_validity(&dom, &path)
    }

    fn number_validity(value: &str) -> Validity {
        input_validity(&format!(r#"<input type="number" value="{value}">"#))
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
    fn readonly_is_ignored_on_checkbox_file_and_select_but_not_text_controls() {
        let flags = input_validity(r#"<input type="checkbox" required readonly>"#);
        assert!(flags.value_missing);

        let flags = input_validity(r#"<input type="file" required readonly>"#);
        assert!(flags.value_missing);

        let flags = select_validity(
            r#"<select required readonly><option value="">Choose</option><option value="x">X</option></select>"#,
        );
        assert!(flags.value_missing);

        let flags = input_validity(r#"<input type="text" required readonly>"#);
        assert!(flags.valid(), "readonly text controls remain barred");
    }

    #[test]
    fn readonly_radio_still_participates_in_group_requiredness() {
        let dom = crate::html::parse_html(
            r#"<form><input id="a" type="radio" name="r" required><input id="b" type="radio" name="r" readonly></form>"#,
        );
        let b = dom_api::get_element_by_id(&dom, "b").unwrap();
        assert!(control_validity(&dom, &b).value_missing);
    }

    #[test]
    fn required_is_inapplicable_to_range_and_color() {
        for kind in ["range", "color"] {
            let flags = input_validity(&format!(r#"<input type="{kind}" required>"#));
            assert!(flags.valid(), "{kind} must not become value-missing");
        }
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
