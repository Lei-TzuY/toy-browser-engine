// ============================================================
//  validation.rs  —  HTML form constraint validation
// ============================================================
//
//  This module is deliberately independent from event dispatch. It answers the
//  pure question "is this live control value valid?"; Document is responsible
//  for the interactive-validation lifecycle (`invalid`, focus, then submit).

use crate::dom::{ElementData, Node};
use crate::forms;
use crate::script::dom_api::{self, NodePath};

/// The validity flags this educational engine currently models.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Validity {
    pub value_missing: bool,
    pub pattern_mismatch: bool,
    pub range_underflow: bool,
    pub range_overflow: bool,
    pub too_short: bool,
    pub too_long: bool,
}

impl Validity {
    pub fn valid(self) -> bool {
        !self.value_missing
            && !self.pattern_mismatch
            && !self.range_underflow
            && !self.range_overflow
            && !self.too_short
            && !self.too_long
    }
}

/// Controls barred from constraint validation never block a submission.
pub fn will_validate(element: &ElementData) -> bool {
    if !element.is_form_control() || element.is_disabled() || element.is_readonly() {
        return false;
    }
    if element.tag_name == "button" {
        return false;
    }
    if element.tag_name == "input" {
        return !matches!(
            element.input_type().as_str(),
            "hidden" | "submit" | "reset" | "button" | "image"
        );
    }
    true
}

/// Compute validity from the control's *live* value and checked state.
pub fn control_validity(dom: &Node, path: &[usize]) -> Validity {
    let Some(element) = dom_api::node_at(dom, path).and_then(|node| node.as_element()) else {
        return Validity::default();
    };
    if !will_validate(element) {
        return Validity::default();
    }

    let value = element.control_value();
    let input_type = element.input_type();
    let required = element.get_attr("required").is_some();

    let value_missing = if !required {
        false
    } else if element.tag_name == "input" && input_type == "checkbox" {
        !element.is_checked()
    } else if element.tag_name == "input" && input_type == "radio" {
        !radio_group_checked(dom, path, element)
    } else {
        value.is_empty()
    };

    let pattern_mismatch = !value.is_empty()
        && element.tag_name == "input"
        && matches!(
            input_type.as_str(),
            "text" | "search" | "tel" | "url" | "email" | "password"
        )
        && element
            .get_attr("pattern")
            .is_some_and(|pattern| !pattern_matches(pattern, &value));

    let number = if element.tag_name == "input" && input_type == "number" {
        value.trim().parse::<f64>().ok()
    } else {
        None
    };
    let min = element
        .get_attr("min")
        .and_then(|text| text.trim().parse::<f64>().ok());
    let max = element
        .get_attr("max")
        .and_then(|text| text.trim().parse::<f64>().ok());
    let range_underflow = number.zip(min).is_some_and(|(value, min)| value < min);
    let range_overflow = number.zip(max).is_some_and(|(value, max)| value > max);

    let length = value.chars().count();
    let min_length = element
        .get_attr("minlength")
        .and_then(|text| text.trim().parse::<usize>().ok());
    let max_length = element
        .get_attr("maxlength")
        .and_then(|text| text.trim().parse::<usize>().ok());
    let length_constrained = element.is_text_entry() && !value.is_empty();
    let too_short = length_constrained && min_length.is_some_and(|min| length < min);
    let too_long = length_constrained && max_length.is_some_and(|max| length > max);

    Validity {
        value_missing,
        pattern_mismatch,
        range_underflow,
        range_overflow,
        too_short,
        too_long,
    }
}

/// Every invalid control in document order.
pub fn invalid_controls(dom: &Node, form_path: &[usize]) -> Vec<NodePath> {
    forms::form_controls(dom, form_path)
        .into_iter()
        .filter(|path| !control_validity(dom, path).valid())
        .collect()
}

fn radio_group_checked(dom: &Node, path: &[usize], element: &ElementData) -> bool {
    let Some(name) = element.get_attr("name").filter(|name| !name.is_empty()) else {
        return element.is_checked();
    };
    let scope = forms::owning_form(dom, path).unwrap_or_default();
    forms::form_controls(dom, &scope)
        .into_iter()
        .filter_map(|candidate| dom_api::node_at(dom, &candidate)?.as_element())
        .any(|other| {
            other.tag_name == "input"
                && other.input_type() == "radio"
                && other.get_attr("name") == Some(name)
                && other.is_checked()
        })
}

// ── pattern= ─────────────────────────────────────────────────────────────────
//
// HTML's pattern is a regular expression matched against the entire value. A
// full regex engine would be disproportionate for this project, so this parser
// implements the useful dependency-free subset below:
//
//   literals, ., \d, \w, \s, [abc], [A-Z], [^0-9]
//   ?, *, +, {m}, {m,}, {m,n}
//
// Unsupported syntax (groups, alternation, lookarounds, malformed classes) is
// treated like an invalid pattern attribute and therefore imposes no constraint.

#[derive(Debug, Clone, PartialEq, Eq)]
enum PatternAtom {
    Literal(char),
    Any,
    Digit,
    Word,
    Space,
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
}

impl PatternAtom {
    fn matches(&self, character: char) -> bool {
        match self {
            PatternAtom::Literal(expected) => character == *expected,
            PatternAtom::Any => true,
            PatternAtom::Digit => character.is_ascii_digit(),
            PatternAtom::Word => character.is_ascii_alphanumeric() || character == '_',
            PatternAtom::Space => character.is_whitespace(),
            PatternAtom::Class { negated, ranges } => {
                let inside = ranges
                    .iter()
                    .any(|(start, end)| *start <= character && character <= *end);
                inside != *negated
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PatternPiece {
    atom: PatternAtom,
    min: usize,
    max: Option<usize>,
}

fn pattern_matches(pattern: &str, value: &str) -> bool {
    // These bounds make the backtracking matcher unsuitable as a DoS primitive.
    if pattern.chars().count() > 256 || value.chars().count() > 4096 {
        return true;
    }
    let pattern = pattern.strip_prefix('^').unwrap_or(pattern);
    let pattern = pattern.strip_suffix('$').unwrap_or(pattern);
    let Some(pieces) = parse_pattern(pattern) else {
        return true;
    };
    let characters: Vec<char> = value.chars().collect();
    match_pattern(&pieces, &characters, 0, 0)
}

fn parse_pattern(pattern: &str) -> Option<Vec<PatternPiece>> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut index = 0;
    let mut pieces = Vec::new();

    while index < chars.len() {
        let atom = match chars[index] {
            '(' | ')' | '|' => return None,
            '.' => {
                index += 1;
                PatternAtom::Any
            }
            '\\' => {
                index += 1;
                let escaped = *chars.get(index)?;
                index += 1;
                match escaped {
                    'd' => PatternAtom::Digit,
                    'w' => PatternAtom::Word,
                    's' => PatternAtom::Space,
                    other => PatternAtom::Literal(other),
                }
            }
            '[' => {
                let (atom, next) = parse_class(&chars, index + 1)?;
                index = next;
                atom
            }
            '*' | '+' | '?' | '{' | '}' => return None,
            literal => {
                index += 1;
                PatternAtom::Literal(literal)
            }
        };

        let (min, max) = match chars.get(index).copied() {
            Some('?') => {
                index += 1;
                (0, Some(1))
            }
            Some('*') => {
                index += 1;
                (0, None)
            }
            Some('+') => {
                index += 1;
                (1, None)
            }
            Some('{') => {
                let (min, max, next) = parse_counted_quantifier(&chars, index + 1)?;
                index = next;
                (min, max)
            }
            _ => (1, Some(1)),
        };
        pieces.push(PatternPiece { atom, min, max });
    }
    Some(pieces)
}

fn parse_class(chars: &[char], mut index: usize) -> Option<(PatternAtom, usize)> {
    let negated = chars.get(index) == Some(&'^');
    if negated {
        index += 1;
    }
    let mut ranges = Vec::new();
    let mut saw_item = false;

    while index < chars.len() {
        if chars[index] == ']' && saw_item {
            return Some((PatternAtom::Class { negated, ranges }, index + 1));
        }
        let start = parse_class_character(chars, &mut index)?;
        saw_item = true;
        if chars.get(index) == Some(&'-') && chars.get(index + 1).is_some_and(|c| *c != ']') {
            index += 1;
            let end = parse_class_character(chars, &mut index)?;
            if end < start {
                return None;
            }
            ranges.push((start, end));
        } else {
            ranges.push((start, start));
        }
    }
    None
}

fn parse_class_character(chars: &[char], index: &mut usize) -> Option<char> {
    let character = *chars.get(*index)?;
    *index += 1;
    if character == '\\' {
        let escaped = *chars.get(*index)?;
        *index += 1;
        Some(escaped)
    } else {
        Some(character)
    }
}

fn parse_counted_quantifier(
    chars: &[char],
    start: usize,
) -> Option<(usize, Option<usize>, usize)> {
    let end = chars[start..].iter().position(|c| *c == '}')? + start;
    let text: String = chars[start..end].iter().collect();
    let (min, max) = if let Some((left, right)) = text.split_once(',') {
        let min = left.parse::<usize>().ok()?;
        let max = if right.is_empty() {
            None
        } else {
            Some(right.parse::<usize>().ok()?)
        };
        if max.is_some_and(|max| max < min) {
            return None;
        }
        (min, max)
    } else {
        let exact = text.parse::<usize>().ok()?;
        (exact, Some(exact))
    };
    if min > 4096 || max.is_some_and(|max| max > 4096) {
        return None;
    }
    Some((min, max, end + 1))
}

fn match_pattern(
    pieces: &[PatternPiece],
    value: &[char],
    piece_index: usize,
    value_index: usize,
) -> bool {
    if piece_index == pieces.len() {
        return value_index == value.len();
    }
    let piece = &pieces[piece_index];
    let hard_max = piece
        .max
        .unwrap_or_else(|| value.len().saturating_sub(value_index));
    let mut positions = vec![value_index];
    let mut cursor = value_index;

    for _ in 0..hard_max {
        let Some(character) = value.get(cursor) else {
            break;
        };
        if !piece.atom.matches(*character) {
            break;
        }
        cursor += 1;
        positions.push(cursor);
    }
    let matched = positions.len() - 1;
    if matched < piece.min {
        return false;
    }

    (piece.min..=matched).rev().any(|count| {
        match_pattern(pieces, value, piece_index + 1, positions[count])
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::parse_html;

    fn path(dom: &Node, id: &str) -> NodePath {
        dom_api::get_element_by_id(dom, id).expect("control")
    }

    fn form(dom: &Node) -> NodePath {
        dom_api::query_selector(dom, &[], "form").expect("form")
    }

    #[test]
    fn required_text_checkbox_and_radio_groups_are_checked() {
        let dom = parse_html(
            r#"<form>
                 <input id="text" required>
                 <input id="box" type="checkbox" required>
                 <input id="r1" type="radio" name="pick" required>
                 <input id="r2" type="radio" name="pick" checked>
               </form>"#,
        );
        assert!(control_validity(&dom, &path(&dom, "text")).value_missing);
        assert!(control_validity(&dom, &path(&dom, "box")).value_missing);
        assert!(control_validity(&dom, &path(&dom, "r1")).valid());
        assert_eq!(invalid_controls(&dom, &form(&dom)).len(), 2);
    }

    #[test]
    fn pattern_is_a_full_match() {
        let dom = parse_html(
            r#"<form>
                 <input id="ok" pattern="[A-Z]{2}\d{3}" value="AB123">
                 <input id="bad" pattern="[A-Z]{2}\d{3}" value="A123">
               </form>"#,
        );
        assert!(control_validity(&dom, &path(&dom, "ok")).valid());
        assert!(control_validity(&dom, &path(&dom, "bad")).pattern_mismatch);
    }

    #[test]
    fn number_min_and_max_set_range_flags() {
        let dom = parse_html(
            r#"<form>
                 <input id="low" type="number" min="10" value="9">
                 <input id="ok" type="number" min="10" max="20" value="15">
                 <input id="high" type="number" max="20" value="21">
               </form>"#,
        );
        assert!(control_validity(&dom, &path(&dom, "low")).range_underflow);
        assert!(control_validity(&dom, &path(&dom, "ok")).valid());
        assert!(control_validity(&dom, &path(&dom, "high")).range_overflow);
    }

    #[test]
    fn length_constraints_use_the_live_text_value() {
        let dom = parse_html(
            r#"<form>
                 <input id="short" minlength="3" value="ab">
                 <input id="long" maxlength="3" value="abcd">
               </form>"#,
        );
        assert!(control_validity(&dom, &path(&dom, "short")).too_short);
        assert!(control_validity(&dom, &path(&dom, "long")).too_long);
    }

    #[test]
    fn disabled_readonly_hidden_and_buttons_do_not_validate() {
        let dom = parse_html(
            r#"<form>
                 <input id="disabled" required disabled>
                 <input id="readonly" required readonly>
                 <input id="hidden" type="hidden" required>
                 <button id="button" required></button>
               </form>"#,
        );
        assert!(invalid_controls(&dom, &form(&dom)).is_empty());
    }

    #[test]
    fn malformed_or_unsupported_patterns_do_not_make_a_control_invalid() {
        let dom = parse_html(
            r#"<form><input id="x" pattern="(a|b)" value="z"></form>"#,
        );
        assert!(control_validity(&dom, &path(&dom, "x")).valid());
    }
}
