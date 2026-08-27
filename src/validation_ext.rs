// ============================================================
//  validation_ext.rs — path-aware validation facade
// ============================================================
//
// The core validity algorithms remain in validation.rs. This facade adds DOM
// relationships and input states that cannot be answered from ElementData
// alone: inherited fieldset disabledness, radio-button group-wide requiredness,
// and calendar/time-aware date/month/week/time/datetime-local constraints.

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

    if element.tag_name == "input" {
        match element.input_type().as_str() {
            "date" => apply_date_validity(element, &mut validity),
            "month" => apply_month_validity(element, &mut validity),
            "week" => apply_week_validity(element, &mut validity),
            "time" => apply_time_validity(element, &mut validity),
            "datetime-local" => apply_datetime_local_validity(element, &mut validity),
            _ => {}
        }
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
    validity.step_mismatch = discrete_step_mismatch(element, value, step_base, 1.0);
}

/// Parse an HTML valid date string into whole days from 1970-01-01.
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

// ── type=month ───────────────────────────────────────────────────────────────

/// Apply Month-state bad-input/range/step rules.
///
/// Month values are represented as whole months from January 1970, which makes
/// ordering and step checks exact and avoids any dependence on month length.
fn apply_month_validity(element: &ElementData, validity: &mut Validity) {
    let raw = element.control_value();
    if raw.is_empty() {
        return;
    }

    let value = parse_month_index(&raw);
    validity.bad_input = value.is_none();
    let Some(value) = value else {
        validity.range_underflow = false;
        validity.range_overflow = false;
        validity.step_mismatch = false;
        return;
    };

    let min = element.get_attr("min").and_then(parse_month_index);
    let max = element.get_attr("max").and_then(parse_month_index);
    validity.range_underflow = min.is_some_and(|minimum| value < minimum);
    validity.range_overflow = max.is_some_and(|maximum| value > maximum);

    let step_base = min
        .or_else(|| element.get_attr("value").and_then(parse_month_index))
        .unwrap_or(0);
    validity.step_mismatch = discrete_step_mismatch(element, value, step_base, 1.0);
}

/// Parse an HTML valid month string (`YYYY-MM`) to months from 1970-01.
fn parse_month_index(text: &str) -> Option<i64> {
    if text.trim() != text || !text.is_ascii() {
        return None;
    }
    let mut parts = text.split('-');
    let year_text = parts.next()?;
    let month_text = parts.next()?;
    if parts.next().is_some()
        || year_text.len() < 4
        || month_text.len() != 2
        || !year_text.bytes().all(|byte| byte.is_ascii_digit())
        || !month_text.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }

    let year = year_text.parse::<i64>().ok()?;
    let month = month_text.parse::<i64>().ok()?;
    if year == 0 || !(1..=12).contains(&month) {
        return None;
    }
    Some((year - 1970).checked_mul(12)? + (month - 1))
}

// ── type=week ────────────────────────────────────────────────────────────────

/// Apply Week-state bad-input/range/step rules.
///
/// Week values are normalized to whole ISO weeks from `1970-W01`. That week
/// starts on Monday 1969-12-29, which is HTML's special default step base for
/// Week-state inputs. Expressing values on this axis turns that base into zero.
fn apply_week_validity(element: &ElementData, validity: &mut Validity) {
    let raw = element.control_value();
    if raw.is_empty() {
        return;
    }

    let value = parse_week_index(&raw);
    validity.bad_input = value.is_none();
    let Some(value) = value else {
        validity.range_underflow = false;
        validity.range_overflow = false;
        validity.step_mismatch = false;
        return;
    };

    let min = element.get_attr("min").and_then(parse_week_index);
    let max = element.get_attr("max").and_then(parse_week_index);
    validity.range_underflow = min.is_some_and(|minimum| value < minimum);
    validity.range_overflow = max.is_some_and(|maximum| value > maximum);

    let step_base = min
        .or_else(|| element.get_attr("value").and_then(parse_week_index))
        .unwrap_or(0);
    validity.step_mismatch = discrete_step_mismatch(element, value, step_base, 1.0);
}

/// Parse an HTML valid week string (`YYYY-Www`) into ISO weeks from 1970-W01.
fn parse_week_index(text: &str) -> Option<i64> {
    if text.trim() != text || !text.is_ascii() {
        return None;
    }
    let (year_text, week_text) = text.split_once("-W")?;
    if year_text.len() < 4
        || week_text.len() != 2
        || !year_text.bytes().all(|byte| byte.is_ascii_digit())
        || !week_text.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }

    let year = year_text.parse::<i64>().ok()?;
    let week = week_text.parse::<i64>().ok()?;
    if year == 0 || week == 0 || week > iso_weeks_in_year(year)? {
        return None;
    }

    let monday = iso_week1_monday(year)?.checked_add((week - 1).checked_mul(7)?)?;
    // 1970-W01 began three days before the Unix epoch. Every valid week
    // Monday is therefore exactly divisible on this shifted day axis.
    Some(monday.checked_add(3)?.div_euclid(7))
}

/// Days since 1970-01-01 for the Monday beginning ISO week 1 of `year`.
fn iso_week1_monday(year: i64) -> Option<i64> {
    if year <= 0 {
        return None;
    }
    let jan4 = days_from_civil(year, 1, 4);
    // 1970-01-01 was Thursday. With Monday=0, Thursday has index 3.
    let weekday = (jan4 + 3).rem_euclid(7);
    jan4.checked_sub(weekday)
}

fn iso_weeks_in_year(year: i64) -> Option<i64> {
    let this = iso_week1_monday(year)?;
    let next = iso_week1_monday(year.checked_add(1)?)?;
    Some((next - this) / 7)
}

// ── type=time ────────────────────────────────────────────────────────────────

/// Apply Time-state bad-input/range/step rules.
///
/// The normalized value is milliseconds since midnight. Time is the only input
/// state here with a periodic domain: when both valid bounds exist and
/// `min > max`, the allowed range wraps across midnight. Values in the gap are
/// simultaneously below the minimum and above the maximum per HTML.
fn apply_time_validity(element: &ElementData, validity: &mut Validity) {
    let raw = element.control_value();
    if raw.is_empty() {
        return;
    }

    let value = parse_time_millis(&raw);
    validity.bad_input = value.is_none();
    let Some(value) = value else {
        validity.range_underflow = false;
        validity.range_overflow = false;
        validity.step_mismatch = false;
        return;
    };

    let min = element.get_attr("min").and_then(parse_time_millis);
    let max = element.get_attr("max").and_then(parse_time_millis);
    if let (Some(minimum), Some(maximum)) = (min, max) {
        if maximum < minimum {
            let gap = value > maximum && value < minimum;
            validity.range_underflow = gap;
            validity.range_overflow = gap;
        } else {
            validity.range_underflow = value < minimum;
            validity.range_overflow = value > maximum;
        }
    } else {
        validity.range_underflow = min.is_some_and(|minimum| value < minimum);
        validity.range_overflow = max.is_some_and(|maximum| value > maximum);
    }

    let step_base = min
        .or_else(|| element.get_attr("value").and_then(parse_time_millis))
        .unwrap_or(0);
    validity.step_mismatch = time_step_mismatch(element, value, step_base);
}

/// Parse an HTML valid time string to milliseconds since midnight.
///
/// Accepted forms are `HH:MM`, optionally `:SS`, and optionally a one- to
/// three-digit fractional second after seconds. Leap seconds are not allowed.
fn parse_time_millis(text: &str) -> Option<i64> {
    if text.trim() != text || !text.is_ascii() {
        return None;
    }
    let mut parts = text.split(':');
    let hour_text = parts.next()?;
    let minute_text = parts.next()?;
    let second_field = parts.next();
    if parts.next().is_some()
        || hour_text.len() != 2
        || minute_text.len() != 2
        || !hour_text.bytes().all(|byte| byte.is_ascii_digit())
        || !minute_text.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }

    let hour = hour_text.parse::<i64>().ok()?;
    let minute = minute_text.parse::<i64>().ok()?;
    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) {
        return None;
    }

    let (second, millis) = match second_field {
        None => (0, 0),
        Some(field) => {
            let (second_text, fraction) = match field.split_once('.') {
                Some((second, fraction)) => (second, Some(fraction)),
                None => (field, None),
            };
            if second_text.len() != 2
                || !second_text.bytes().all(|byte| byte.is_ascii_digit())
            {
                return None;
            }
            let second = second_text.parse::<i64>().ok()?;
            if !(0..=59).contains(&second) {
                return None;
            }
            let millis = match fraction {
                None => 0,
                Some(fraction) => {
                    if fraction.is_empty()
                        || fraction.len() > 3
                        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
                    {
                        return None;
                    }
                    let raw = fraction.parse::<i64>().ok()?;
                    match fraction.len() {
                        1 => raw * 100,
                        2 => raw * 10,
                        3 => raw,
                        _ => unreachable!(),
                    }
                }
            };
            (second, millis)
        }
    };

    Some((((hour * 60 + minute) * 60 + second) * 1000) + millis)
}

/// Step mismatch for millisecond-valued input states whose `step` is seconds.
fn time_step_mismatch(element: &ElementData, value: i64, base: i64) -> bool {
    let step_attribute = element.get_attr("step").map(str::trim);
    if step_attribute.is_some_and(|step| step.eq_ignore_ascii_case("any")) {
        return false;
    }
    let step_seconds = step_attribute
        .and_then(|step| step.parse::<f64>().ok())
        .filter(|step| step.is_finite() && *step > 0.0)
        .unwrap_or(60.0);
    let allowed_millis = step_seconds * 1000.0;
    let steps = (value - base) as f64 / allowed_millis;
    let tolerance = 1e-9 * steps.abs().max(1.0);
    (steps - steps.round()).abs() > tolerance
}

// ── type=datetime-local ─────────────────────────────────────────────────────

/// Apply Local Date and Time-state bad-input/range/step rules.
///
/// The numeric axis is abstract local milliseconds from
/// `1970-01-01T00:00:00.000`; no time-zone offset or daylight-saving rule is
/// applied. Unlike Time state, this domain is linear and does not wrap.
fn apply_datetime_local_validity(element: &ElementData, validity: &mut Validity) {
    let raw = element.control_value();
    if raw.is_empty() {
        return;
    }

    let value = parse_datetime_local_millis(&raw);
    validity.bad_input = value.is_none();
    let Some(value) = value else {
        validity.range_underflow = false;
        validity.range_overflow = false;
        validity.step_mismatch = false;
        return;
    };

    let min = element.get_attr("min").and_then(parse_datetime_local_millis);
    let max = element.get_attr("max").and_then(parse_datetime_local_millis);
    validity.range_underflow = min.is_some_and(|minimum| value < minimum);
    validity.range_overflow = max.is_some_and(|maximum| value > maximum);

    let step_base = min
        .or_else(|| element.get_attr("value").and_then(parse_datetime_local_millis))
        .unwrap_or(0);
    validity.step_mismatch = time_step_mismatch(element, value, step_base);
}

/// Parse a valid local date and time string to abstract milliseconds from the
/// local epoch. Both `T` and a single ASCII space are accepted as separators;
/// the real input sanitizer would subsequently normalize the latter to `T`.
fn parse_datetime_local_millis(text: &str) -> Option<i64> {
    if text.trim() != text || !text.is_ascii() {
        return None;
    }
    let separator = text
        .bytes()
        .position(|byte| byte == b'T' || byte == b' ')?;
    let date = &text[..separator];
    let time = &text[separator + 1..];
    if date.is_empty() || time.is_empty() {
        return None;
    }
    let days = parse_date_days(date)?;
    let millis = parse_time_millis(time)?;
    days.checked_mul(86_400_000)?.checked_add(millis)
}

/// Shared step checker for input states whose normalized values are discrete
/// integer units (days for date, months for month, weeks for week).
fn discrete_step_mismatch(element: &ElementData, value: i64, base: i64, default_step: f64) -> bool {
    let step_attribute = element.get_attr("step").map(str::trim);
    if step_attribute.is_some_and(|step| step.eq_ignore_ascii_case("any")) {
        return false;
    }
    let step = step_attribute
        .and_then(|step| step.parse::<f64>().ok())
        .filter(|step| step.is_finite() && *step > 0.0)
        .unwrap_or(default_step);
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

    #[test]
    fn month_parser_normalizes_to_months_from_epoch() {
        assert_eq!(parse_month_index("1970-01"), Some(0));
        assert_eq!(parse_month_index("1970-12"), Some(11));
        assert_eq!(parse_month_index("1971-01"), Some(12));
        assert_eq!(parse_month_index("1969-12"), Some(-1));
        assert!(parse_month_index("2026-13").is_none());
        assert!(parse_month_index("2026-1").is_none());
        assert!(parse_month_index("0000-01").is_none());
    }

    #[test]
    fn month_min_max_and_step_are_calendar_month_based() {
        let flags = validity(
            r#"<input type="month" value="2026-08" min="2026-09" max="2026-12">"#,
        );
        assert!(flags.range_underflow);
        assert!(!flags.range_overflow);

        let flags = validity(
            r#"<input type="month" value="2026-05" min="2026-01" step="3">"#,
        );
        assert!(flags.step_mismatch);

        let flags = validity(
            r#"<input type="month" value="2026-07" min="2026-01" step="3">"#,
        );
        assert!(!flags.step_mismatch);
    }

    #[test]
    fn malformed_nonempty_month_sets_bad_input_and_step_any_bypasses_step() {
        let flags = validity(r#"<input type="month" value="2026-00">"#);
        assert!(flags.bad_input);
        assert!(!flags.valid());

        let flags = validity(r#"<input type="month" value="2026-08" step="any">"#);
        assert!(!flags.step_mismatch);
    }

    #[test]
    fn week_parser_uses_iso_week_year_boundaries() {
        assert_eq!(parse_week_index("1970-W01"), Some(0));
        assert_eq!(parse_week_index("1969-W52"), Some(-1));
        assert_eq!(parse_week_index("1970-W02"), Some(1));
        assert!(parse_week_index("2015-W53").is_some());
        assert!(parse_week_index("2020-W53").is_some());
        assert!(parse_week_index("2014-W53").is_none());
        assert!(parse_week_index("2021-W53").is_none());
        assert!(parse_week_index("2026-W00").is_none());
        assert!(parse_week_index("2026-W1").is_none());
        assert!(parse_week_index("0000-W01").is_none());
    }

    #[test]
    fn week_min_max_and_step_work_across_year_boundaries() {
        let flags = validity(
            r#"<input type="week" value="2025-W51" min="2025-W52" max="2026-W10">"#,
        );
        assert!(flags.range_underflow);
        assert!(!flags.range_overflow);

        let flags = validity(
            r#"<input type="week" value="2026-W01" min="2025-W52" step="2">"#,
        );
        assert!(flags.step_mismatch);

        let flags = validity(
            r#"<input type="week" value="2026-W02" min="2025-W52" step="2">"#,
        );
        assert!(!flags.step_mismatch);
    }

    #[test]
    fn malformed_nonempty_week_sets_bad_input_and_step_any_bypasses_step() {
        let flags = validity(r#"<input type="week" value="2021-W53">"#);
        assert!(flags.bad_input);
        assert!(!flags.valid());

        let flags = validity(r#"<input type="week" value="2026-W35" step="any">"#);
        assert!(!flags.step_mismatch);
    }

    #[test]
    fn time_parser_supports_optional_seconds_and_millisecond_fraction() {
        assert_eq!(parse_time_millis("00:00"), Some(0));
        assert_eq!(parse_time_millis("23:59"), Some(86_340_000));
        assert_eq!(parse_time_millis("12:34:56"), Some(45_296_000));
        assert_eq!(parse_time_millis("12:34:56.5"), Some(45_296_500));
        assert_eq!(parse_time_millis("12:34:56.05"), Some(45_296_050));
        assert_eq!(parse_time_millis("12:34:56.005"), Some(45_296_005));
        assert!(parse_time_millis("24:00").is_none());
        assert!(parse_time_millis("12:60").is_none());
        assert!(parse_time_millis("12:34:60").is_none());
        assert!(parse_time_millis("1:34").is_none());
        assert!(parse_time_millis("12:34:.5").is_none());
        assert!(parse_time_millis("12:34:56.1234").is_none());
    }

    #[test]
    fn reversed_time_range_wraps_midnight_and_gap_has_both_range_flags() {
        for value in ["21:00", "23:30", "00:00", "06:00"] {
            let flags = validity(&format!(
                r#"<input type="time" value="{value}" min="21:00" max="06:00">"#
            ));
            assert!(!flags.range_underflow, "{value}");
            assert!(!flags.range_overflow, "{value}");
        }

        let flags = validity(r#"<input type="time" value="12:00" min="21:00" max="06:00">"#);
        assert!(flags.range_underflow);
        assert!(flags.range_overflow);
    }

    #[test]
    fn time_step_is_seconds_scaled_to_milliseconds() {
        let flags = validity(r#"<input type="time" value="00:00:30" min="00:00">"#);
        assert!(flags.step_mismatch, "default step is 60 seconds from midnight");

        let flags = validity(r#"<input type="time" value="00:00:30" step="1">"#);
        assert!(!flags.step_mismatch);

        let flags = validity(r#"<input type="time" value="00:00:00.500" step="0.5">"#);
        assert!(!flags.step_mismatch);

        let flags = validity(r#"<input type="time" value="12:34:56.789" step="any">"#);
        assert!(!flags.step_mismatch);
    }

    #[test]
    fn malformed_nonempty_time_sets_bad_input_and_normal_ranges_are_linear() {
        let flags = validity(r#"<input type="time" value="25:00">"#);
        assert!(flags.bad_input);
        assert!(!flags.valid());

        let flags = validity(r#"<input type="time" value="08:59" min="09:00" max="17:00">"#);
        assert!(flags.range_underflow);
        assert!(!flags.range_overflow);

        let flags = validity(r#"<input type="time" value="17:01" min="09:00" max="17:00">"#);
        assert!(!flags.range_underflow);
        assert!(flags.range_overflow);
    }

    #[test]
    fn datetime_local_parser_uses_abstract_local_epoch_milliseconds() {
        assert_eq!(parse_datetime_local_millis("1970-01-01T00:00"), Some(0));
        assert_eq!(
            parse_datetime_local_millis("1970-01-02T00:00"),
            Some(86_400_000)
        );
        assert_eq!(
            parse_datetime_local_millis("1969-12-31T23:59"),
            Some(-60_000)
        );
        assert_eq!(
            parse_datetime_local_millis("2000-02-29 12:34:56.789"),
            parse_date_days("2000-02-29")
                .and_then(|days| days.checked_mul(86_400_000))
                .and_then(|base| base.checked_add(45_296_789))
        );
        assert!(parse_datetime_local_millis("1900-02-29T12:00").is_none());
        assert!(parse_datetime_local_millis("2026-08-27T24:00").is_none());
        assert!(parse_datetime_local_millis("2026-08-27").is_none());
    }

    #[test]
    fn datetime_local_range_and_step_are_linear_on_local_milliseconds() {
        let flags = validity(
            r#"<input type="datetime-local" value="2026-08-27T08:59" min="2026-08-27T09:00" max="2026-08-27T17:00">"#,
        );
        assert!(flags.range_underflow);
        assert!(!flags.range_overflow);

        let flags = validity(
            r#"<input type="datetime-local" value="2026-08-27T17:01" min="2026-08-27T09:00" max="2026-08-27T17:00">"#,
        );
        assert!(!flags.range_underflow);
        assert!(flags.range_overflow);

        let flags = validity(
            r#"<input type="datetime-local" value="1970-01-01T00:00:30" min="1970-01-01T00:00">"#,
        );
        assert!(flags.step_mismatch);

        let flags = validity(
            r#"<input type="datetime-local" value="1970-01-01T00:00:00.500" step="0.5">"#,
        );
        assert!(!flags.step_mismatch);
    }

    #[test]
    fn malformed_nonempty_datetime_local_sets_bad_input() {
        let flags = validity(r#"<input type="datetime-local" value="2026-02-30T12:00">"#);
        assert!(flags.bad_input);
        assert!(!flags.valid());
    }
}