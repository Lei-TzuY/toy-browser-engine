// ============================================================
//  validation_ext.rs — path-aware validation facade
// ============================================================
//
// The core validity algorithms remain in validation.rs. This facade adds DOM
// relationships and input states that cannot be answered from ElementData
// alone: inherited fieldset disabledness, radio-button group-wide requiredness,
// and calendar-aware date constraints.

use crate::dom::{ElementData, Node};
use crate::forms;
use crate::script::dom_api::{self, NodePath};

pub use crate::validation_base::{will_validate, Validity};

/// Compute validity for a live control, including structural/group semantics.
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

    if element.tag_name == "input" && element.input_type() == "date" {
        apply_date_validity(element, &mut validity);
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

// ── type=date ────────────────────────────────────────────────────────────────

/// Apply Date-state bad-input/range/step rules to the shared validity object.
///
/// Browsers normally sanitize an invalid date string back to empty. The toy DOM
/// deliberately preserves raw live values, so a non-empty unparsable value is
/// represented as `bad_input`, matching the same strategy used for number.
fn apply_date_validity(element: &ElementData, validity: &mut Validity) {
    let raw = element.control_value();
    if raw.is_empty() {
        return;
    }

    let value = parse_date_days(&raw);
    validity.bad_input = value.is_none();
    let Some(value) = value else {
        validity.range_underflow = false;
        validity.range_overflow = false;
        validity.step_mismatch = false;
        return;
    };

    let min = element.get_attr("min").and_then(parse_date_days);
    let max = element.get_attr("max").and_then(parse_date_days);
    validity.range_underflow = min.is_some_and(|minimum| value < minimum);
    validity.range_overflow = max.is_some_and(|maximum| value > maximum);

    let step_base = min
        .or_else(|| element.get_attr("value").and_then(parse_date_days))
        .unwrap_or(0);
    validity.step_mismatch = date_step_mismatch(element, value, step_base);
}

/// Parse an HTML valid date string into whole days from 1970-01-01.
///
/// The year is four or more ASCII digits and must be greater than zero; month
/// and day are exactly two digits. Gregorian month lengths and leap years are
/// checked before conversion.
fn parse_date_days(text: &str) -> Option<i64> {
    if text.trim() != text || !text.is_ascii() {
        return None;
    }
    let mut parts = text.split('-');
    let year_text = parts.next()?;
    let month_text = parts.next()?;
    let day_text = parts.next()?;
    if parts.next().is_some()
        || year_text.len() < 4
        || month_text.len() != 2
        || day_text.len() != 2
        || !year_text.bytes().all(|byte| byte.is_ascii_digit())
        || !month_text.bytes().all(|byte| byte.is_ascii_digit())
        || !day_text.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }

    let year = year_text.parse::<i64>().ok()?;
    let month = month_text.parse::<u32>().ok()?;
    let day = day_text.parse::<u32>().ok()?;
    if year == 0 || !(1..=12).contains(&month) {
        return None;
    }
    let month_days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => return None,
    };
    if day == 0 || day > month_days {
        return None;
    }
    Some(days_from_civil(year, month, day))
}

fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// Proleptic-Gregorian civil date to days since 1970-01-01.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let shifted_month = i64::from(month) + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn date_step_mismatch(element: &ElementData, value: i64, base: i64) -> bool {
    let step_attribute = element.get_attr("step").map(str::trim);
    if step_attribute.is_some_and(|step| step.eq_ignore_ascii_case("any")) {
        return false;
    }
    let step = step_attribute
        .and_then(|step| step.parse::<f64>().ok())
        .filter(|step| step.is_finite() && *step > 0.0)
        .unwrap_or(1.0);
    let steps = (value - base) as f64 / step;
    let tolerance = 1e-9 * steps.abs().max(1.0);
    (steps - steps.round()).abs() > tolerance
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::parse_html;

    fn validity(html: &str) -> Validity {
        let dom = parse_html(html);
        let path = dom_api::query_selector(&dom, &[], "input").unwrap();
        control_validity(&dom, &path)
    }

    #[test]
    fn date_parser_accepts_real_calendar_dates_and_rejects_impossible_ones() {
        assert_eq!(parse_date_days("1970-01-01"), Some(0));
        assert_eq!(parse_date_days("1970-01-02"), Some(1));
        assert!(parse_date_days("2000-02-29").is_some());
        assert!(parse_date_days("1900-02-29").is_none());
        assert!(parse_date_days("2026-04-31").is_none());
        assert!(parse_date_days("2026-4-01").is_none());
        assert!(parse_date_days("0000-01-01").is_none());
    }

    #[test]
    fn malformed_nonempty_date_sets_bad_input() {
        let flags = validity(r#"<input type="date" value="2026-02-30">"#);
        assert!(flags.bad_input);
        assert!(!flags.valid());
    }

    #[test]
    fn date_min_and_max_set_range_flags() {
        let flags = validity(
            r#"<input type="date" value="2026-08-20" min="2026-08-21" max="2026-08-30">"#,
        );
        assert!(flags.range_underflow);
        assert!(!flags.range_overflow);

        let flags = validity(
            r#"<input type="date" value="2026-09-01" min="2026-08-21" max="2026-08-30">"#,
        );
        assert!(!flags.range_underflow);
        assert!(flags.range_overflow);
    }

    #[test]
    fn date_step_uses_days_and_the_standard_step_base_order() {
        let flags = validity(
            r#"<input type="date" value="1970-01-02" min="1970-01-01" step="2">"#,
        );
        assert!(flags.step_mismatch);

        let flags = validity(
            r#"<input type="date" value="1970-01-03" min="1970-01-01" step="2">"#,
        );
        assert!(!flags.step_mismatch);

        let flags = validity(r#"<input type="date" value="2026-08-27" step="any">"#);
        assert!(!flags.step_mismatch);
    }
}
