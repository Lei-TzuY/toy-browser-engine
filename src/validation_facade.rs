// ============================================================
//  validation_facade.rs — final path-aware validation facade
// ============================================================
//
// Most validation lives in validation_constraints. This final facade overlays
// structural and lexical rules that need stricter live-DOM information than the
// older element-only validators expose: select placeholder selectedness, exact
// readonly applicability, the Number state's valid-floating-point-number syntax,
// and Range-state defaults/constraints.

use crate::dom::{ElementData, Node};
use crate::forms;
use crate::script::dom_api::{self, NodePath};

pub use crate::validation_constraints::Validity;

/// Whether a control is structurally eligible for constraint validation.
///
/// The older validator treats the mere presence of `readonly` as barring every
/// form control. HTML only gives `readonly` that meaning for textarea and the
/// textual/numeric input states that actually support the attribute. Stray
/// `readonly` attributes on checkbox, radio, file, range, color and select are
/// ignored rather than turning those controls into validation escape hatches.
pub fn will_validate(element: &ElementData) -> bool {
    if !element.is_form_control() || element.is_disabled() {
        return false;
    }

    match element.tag_name.as_str() {
        "button" => false,
        "textarea" => !element.is_readonly(),
        "input" => {
            let input_type = element.input_type();
            if matches!(
                input_type.as_str(),
                "hidden" | "submit" | "reset" | "button" | "image"
            ) {
                return false;
            }
            !(element.is_readonly() && readonly_applies_to_input(&input_type))
        }
        // `readonly` does not apply to select. Other form controls that reach
        // this branch are likewise not barred by a stray readonly attribute.
        _ => true,
    }
}

fn readonly_applies_to_input(input_type: &str) -> bool {
    matches!(
        input_type,
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

    let effectively_disabled = forms::is_effectively_disabled(dom, path);
    let participates = will_validate(element) && !effectively_disabled;

    if element.tag_name == "select"
        && element.get_attr("required").is_some()
        && participates
    {
        validity.value_missing = crate::select_state::required_value_missing(dom, path)
            .unwrap_or(validity.value_missing);
    }

    if element.tag_name == "input" {
        match element.input_type().as_str() {
            // `readonly` does not apply to checkboxes. The base validator can
            // therefore incorrectly return an all-valid state for
            // `<input type=checkbox required readonly>`; restore the actual
            // Required-state rule here.
            "checkbox" if participates && element.get_attr("required").is_some() => {
                validity.value_missing = !element.is_checked();
            }
            // The base number validator deliberately keeps a raw live string,
            // but its numeric parser trims whitespace and delegates to Rust's
            // permissive float parser. HTML's Number state is stricter: the
            // value must itself be a valid floating-point-number string. A real
            // browser would sanitize invalid strings back to empty; this engine
            // preserves them while editing, so the equivalent observable state
            // is `bad_input`.
            "number" if participates => {
                let raw = element.control_value();
                if !raw.is_empty() && !valid_number_state_syntax(&raw) {
                    validity.bad_input = true;
                    // Range and step constraints only apply after the value
                    // parses as a Number-state value; do not leak flags computed
                    // from a trimmed or otherwise more permissive host parse.
                    validity.range_underflow = false;
                    validity.range_overflow = false;
                    validity.step_mismatch = false;
                }
            }
            // `required` and `readonly` do not apply to Range state. Disabled
            // controls are still barred normally.
            "range" if !effectively_disabled => apply_range_validity(element, &mut validity),
            _ => {}
        }
    }

    validity
}

/// HTML's valid floating-point-number lexical grammar used by numeric states.
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

/// Parse a live numeric-state value using the strict HTML lexical grammar.
fn parse_live_finite_number(value: &str) -> Option<f64> {
    if !valid_number_state_syntax(value) {
        return None;
    }
    value.parse::<f64>().ok().filter(|number| number.is_finite())
}

/// Parse a numeric content attribute using the engine's existing forgiving
/// attribute convention: surrounding whitespace and a non-conforming leading
/// plus are tolerated by the parsing algorithm, but the whole remaining token
/// must still be a finite host number.
fn parse_numeric_attribute(value: &str) -> Option<f64> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|number| number.is_finite())
}

/// Overlay Range-state validation on top of the older generic validator.
///
/// A browser normally sanitizes/clamps a range value so malformed or out-of-
/// range strings are difficult to observe. This engine deliberately preserves
/// arbitrary raw live strings, like its Number/Date implementations, so we
/// expose the corresponding bad-input/range/step flags instead of silently
/// rewriting DOM state here. Full Range value sanitization remains separate.
fn apply_range_validity(element: &ElementData, validity: &mut Validity) {
    // Range always has a value in the browser model; `required` does not apply.
    validity.value_missing = false;

    let raw = element.control_value();
    if raw.is_empty() {
        // The real Range sanitizer would replace this with its default value.
        // Until DOM sanitization exists, an empty raw value must at least not be
        // mistaken for a Required-state failure.
        validity.bad_input = false;
        validity.range_underflow = false;
        validity.range_overflow = false;
        validity.step_mismatch = false;
        return;
    }

    let value = parse_live_finite_number(&raw);
    validity.bad_input = value.is_none();
    let Some(value) = value else {
        validity.range_underflow = false;
        validity.range_overflow = false;
        validity.step_mismatch = false;
        return;
    };

    // Range, unlike Number, has actual default bounds.
    let min = element
        .get_attr("min")
        .and_then(parse_numeric_attribute)
        .unwrap_or(0.0);
    let max = element
        .get_attr("max")
        .and_then(parse_numeric_attribute)
        .unwrap_or(100.0);
    validity.range_underflow = value < min;
    validity.range_overflow = value > max;

    // Generic step-base precedence is an explicit valid min attribute, then a
    // valid content value, then zero. The default Range step is one.
    let step_base = element
        .get_attr("min")
        .and_then(parse_numeric_attribute)
        .or_else(|| element.get_attr("value").and_then(parse_numeric_attribute))
        .unwrap_or(0.0);
    validity.step_mismatch = range_step_mismatch(element, value, step_base);
}

fn range_step_mismatch(element: &ElementData, value: f64, base: f64) -> bool {
    let step_attribute = element.get_attr("step").map(str::trim);
    if step_attribute.is_some_and(|step| step.eq_ignore_ascii_case("any")) {
        return false;
    }
    let step = step_attribute
        .and_then(parse_numeric_attribute)
        .filter(|step| *step > 0.0)
        .unwrap_or(1.0);
    let steps = (value - base) / step;
    if !steps.is_finite() {
        return false;
    }
    let tolerance = 1e-9 * steps.abs().max(1.0);
    (steps - steps.round()).abs() > tolerance
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

    fn input_element(html: &str) -> ElementData {
        let dom = parse_html(html);
        dom_api::query_selector(&dom, &[], "input")
            .and_then(|path| dom_api::node_at(&dom, &path))
            .and_then(|node| node.as_element())
            .expect("input")
            .clone()
    }

    fn number_validity(value: &str) -> Validity {
        input_validity(&format!(r#"<input type="number" value="{value}">"#))
    }

    #[test]
    fn readonly_only_bars_the_input_states_that_support_it() {
        for input_type in [
            "text",
            "search",
            "tel",
            "url",
            "email",
            "password",
            "date",
            "month",
            "week",
            "time",
            "datetime-local",
            "number",
        ] {
            let element = input_element(&format!(r#"<input type="{input_type}" readonly>"#));
            assert!(!will_validate(&element), "{input_type} should be readonly-barred");
        }

        for input_type in ["checkbox", "radio", "file", "range", "color"] {
            let element = input_element(&format!(r#"<input type="{input_type}" readonly>"#));
            assert!(will_validate(&element), "readonly must not bar {input_type}");
        }
    }

    #[test]
    fn readonly_does_not_bar_select_validation() {
        let dom = parse_html(r#"<select id="s" readonly><option>One</option></select>"#);
        let path = dom_api::get_element_by_id(&dom, "s").unwrap();
        let element = dom_api::node_at(&dom, &path).unwrap().as_element().unwrap();
        assert!(will_validate(element));
    }

    #[test]
    fn readonly_required_checkbox_still_suffers_value_missing() {
        let flags = input_validity(r#"<input type="checkbox" required readonly>"#);
        assert!(flags.value_missing);

        let flags = input_validity(r#"<input type="checkbox" required readonly checked>"#);
        assert!(!flags.value_missing);
    }

    #[test]
    fn readonly_required_select_still_uses_placeholder_semantics() {
        let flags = select_validity(
            r#"<select required readonly><option value="">Choose</option><option value="x">X</option></select>"#,
        );
        assert!(flags.value_missing);
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

    #[test]
    fn range_required_and_readonly_do_not_bar_or_require_the_control() {
        let flags = input_validity(r#"<input type="range" required readonly min="10" value="5">"#);
        assert!(!flags.value_missing);
        assert!(flags.range_underflow, "readonly does not apply to Range state");
    }

    #[test]
    fn range_uses_zero_and_one_hundred_as_default_bounds() {
        let flags = input_validity(r#"<input type="range" value="-1">"#);
        assert!(flags.range_underflow);
        assert!(!flags.range_overflow);

        let flags = input_validity(r#"<input type="range" value="101">"#);
        assert!(!flags.range_underflow);
        assert!(flags.range_overflow);

        assert!(input_validity(r#"<input type="range" value="50">"#).valid());
    }

    #[test]
    fn range_step_uses_min_then_content_value_as_its_base() {
        let flags = input_validity(r#"<input type="range" min="0.5" max="10" step="2" value="4.5">"#);
        assert!(flags.valid(), "min-based grid should accept 4.5: {flags:?}");

        let flags = input_validity(r#"<input type="range" min="0.5" max="10" step="2" value="3.5">"#);
        assert!(flags.step_mismatch);

        let flags = input_validity(r#"<input type="range" max="10" step="2" value="0.5">"#);
        assert!(flags.valid(), "content value defines the base when min is absent");
    }

    #[test]
    fn malformed_range_raw_value_sets_only_bad_input() {
        let flags = input_validity(r#"<input type="range" required min="20" max="30" step="3" value="+12">"#);
        assert!(flags.bad_input);
        assert!(!flags.value_missing);
        assert!(!flags.range_underflow);
        assert!(!flags.range_overflow);
        assert!(!flags.step_mismatch);
    }

    #[test]
    fn disabled_range_remains_barred_from_validation() {
        let flags = input_validity(r#"<input type="range" disabled min="20" value="5">"#);
        assert!(flags.valid());
    }
}
