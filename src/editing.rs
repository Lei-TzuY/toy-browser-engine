// ============================================================
//  editing.rs  —  Text editing commands for form controls
// ============================================================
//
//  Pure operations over an element's live value and caret. The event layer
//  decides *when* to run a command (after a `keydown` that nothing cancelled);
//  this module decides what it does.
//
//  Values are edited in characters, not bytes, so multi-byte text behaves.

use crate::dom::ElementData;
use crate::text::measure_text;

/// One editing operation, already resolved from a key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditCommand {
    Insert(char),
    /// Enter inside a `<textarea>`.
    InsertNewline,
    Backspace,
    Delete,
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    MoveToStart,
    MoveToEnd,
}

impl EditCommand {
    /// True when the command changes the value rather than just the caret.
    pub fn mutates_value(&self) -> bool {
        matches!(
            self,
            EditCommand::Insert(_)
                | EditCommand::InsertNewline
                | EditCommand::Backspace
                | EditCommand::Delete
        )
    }
}

/// What running a command did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EditResult {
    /// The value changed, so an `input` event is due.
    pub value_changed: bool,
    /// The caret moved (with or without a value change).
    pub caret_moved: bool,
}

impl EditResult {
    pub fn did_nothing(&self) -> bool {
        !self.value_changed && !self.caret_moved
    }
}

/// The most characters this control accepts, from `maxlength`.
///
/// HTML only applies `maxlength` to textarea and the textual input states.
/// Number remains keyboard-editable in this engine, but its length is governed
/// by numeric constraints (`min`/`max`/`step`), never by character count.
fn max_length(element: &ElementData) -> Option<usize> {
    let applies = match element.tag_name.as_str() {
        "textarea" => true,
        "input" => matches!(
            element.input_type().as_str(),
            "text" | "search" | "url" | "tel" | "email" | "password"
        ),
        _ => false,
    };
    applies
        .then(|| element.get_attr("maxlength"))
        .flatten()?
        .trim()
        .parse::<usize>()
        .ok()
}

/// Apply `command` to a text-entry element.
///
/// Returns what changed. Disabled and read-only controls accept caret
/// movement but never value changes, matching the HTML behaviour.
pub fn apply(element: &mut ElementData, command: EditCommand) -> EditResult {
    let mut result = EditResult::default();
    if !element.is_text_entry() {
        return result;
    }
    let editable = !element.is_disabled() && !element.is_readonly();
    if command.mutates_value() && !editable {
        return result;
    }

    let mut characters: Vec<char> = element.control_value().chars().collect();
    let caret = element.caret().min(characters.len());
    let multiline = element.tag_name == "textarea";

    let new_caret = match command {
        EditCommand::Insert(c) => {
            if max_length(element).is_some_and(|limit| characters.len() >= limit) {
                return result;
            }
            characters.insert(caret, c);
            result.value_changed = true;
            caret + 1
        }
        EditCommand::InsertNewline => {
            if !multiline {
                return result;
            }
            if max_length(element).is_some_and(|limit| characters.len() >= limit) {
                return result;
            }
            characters.insert(caret, '\n');
            result.value_changed = true;
            caret + 1
        }
        EditCommand::Backspace => {
            if caret == 0 {
                return result;
            }
            characters.remove(caret - 1);
            result.value_changed = true;
            caret - 1
        }
        EditCommand::Delete => {
            if caret >= characters.len() {
                return result;
            }
            characters.remove(caret);
            result.value_changed = true;
            caret
        }
        EditCommand::MoveLeft => caret.saturating_sub(1),
        EditCommand::MoveRight => (caret + 1).min(characters.len()),
        EditCommand::MoveToStart => {
            if multiline {
                line_start(&characters, caret)
            } else {
                0
            }
        }
        EditCommand::MoveToEnd => {
            if multiline {
                line_end(&characters, caret)
            } else {
                characters.len()
            }
        }
        EditCommand::MoveUp => {
            if !multiline {
                return result;
            }
            move_line(&characters, caret, -1)
        }
        EditCommand::MoveDown => {
            if !multiline {
                return result;
            }
            move_line(&characters, caret, 1)
        }
    };

    if result.value_changed {
        element.set_control_value(characters.into_iter().collect::<String>());
    }
    result.caret_moved = new_caret != caret;
    element.set_caret(new_caret);
    result
}

/// Index of the first character on the caret's line.
fn line_start(characters: &[char], caret: usize) -> usize {
    characters[..caret]
        .iter()
        .rposition(|c| *c == '\n')
        .map(|index| index + 1)
        .unwrap_or(0)
}

/// Index just past the last character on the caret's line.
fn line_end(characters: &[char], caret: usize) -> usize {
    characters[caret..]
        .iter()
        .position(|c| *c == '\n')
        .map(|offset| caret + offset)
        .unwrap_or(characters.len())
}

/// Move the caret one line up (`-1`) or down (`1`), keeping its column.
fn move_line(characters: &[char], caret: usize, direction: i32) -> usize {
    let start = line_start(characters, caret);
    let column = caret - start;

    if direction < 0 {
        if start == 0 {
            return caret;
        }
        let previous_start = line_start(characters, start - 1);
        // `start - 1` is the newline itself, so it bounds the previous line.
        (previous_start + column).min(start - 1)
    } else {
        let end = line_end(characters, caret);
        if end >= characters.len() {
            return caret;
        }
        let next_start = end + 1;
        let next_end = line_end(characters, next_start);
        (next_start + column).min(next_end)
    }
}

/// Caret index for a click `offset` pixels into the value.
///
/// Picks the character boundary nearest the click, so clicking the right half
/// of a glyph puts the caret after it.
pub fn caret_for_offset(value: &str, font_size: f32, offset: f32) -> usize {
    if offset <= 0.0 {
        return 0;
    }
    let characters: Vec<char> = value.chars().collect();
    let mut best = 0;
    let mut best_distance = f32::MAX;

    for index in 0..=characters.len() {
        let prefix: String = characters[..index].iter().collect();
        let distance = (measure_text(&prefix, font_size) - offset).abs();
        if distance < best_distance {
            best_distance = distance;
            best = index;
        }
    }
    best
}

/// Split a value into the lines a `<textarea>` displays.
pub fn value_lines(value: &str) -> Vec<&str> {
    value.split('\n').collect()
}

/// `(line index, column)` of a caret inside a multi-line value.
pub fn caret_line_column(value: &str, caret: usize) -> (usize, usize) {
    let mut line = 0;
    let mut column = 0;
    for (index, character) in value.chars().enumerate() {
        if index == caret {
            return (line, column);
        }
        if character == '\n' {
            line += 1;
            column = 0;
        } else {
            column += 1;
        }
    }
    (line, column)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn text_input(attributes: &[(&str, &str)]) -> ElementData {
        ElementData::new(
            "input",
            attributes
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    fn textarea() -> ElementData {
        ElementData::new("textarea", vec![])
    }

    fn type_text(element: &mut ElementData, text: &str) {
        for c in text.chars() {
            apply(element, EditCommand::Insert(c));
        }
    }

    #[test]
    fn typing_inserts_at_the_caret() {
        let mut input = text_input(&[]);
        type_text(&mut input, "abc");
        assert_eq!(input.control_value(), "abc");
        assert_eq!(input.caret(), 3);

        apply(&mut input, EditCommand::MoveLeft);
        apply(&mut input, EditCommand::Insert('X'));
        assert_eq!(input.control_value(), "abXc");
        assert_eq!(input.caret(), 3);
    }

    #[test]
    fn backspace_and_delete_remove_around_the_caret() {
        let mut input = text_input(&[]);
        type_text(&mut input, "abcd");
        apply(&mut input, EditCommand::Backspace);
        assert_eq!(input.control_value(), "abc");

        apply(&mut input, EditCommand::MoveToStart);
        apply(&mut input, EditCommand::Delete);
        assert_eq!(input.control_value(), "bc");
        assert_eq!(input.caret(), 0);

        // Both are no-ops at the ends of the value.
        assert!(apply(&mut input, EditCommand::Backspace).did_nothing());
        apply(&mut input, EditCommand::MoveToEnd);
        assert!(apply(&mut input, EditCommand::Delete).did_nothing());
    }

    #[test]
    fn caret_moves_without_changing_the_value() {
        let mut input = text_input(&[]);
        type_text(&mut input, "hello");
        let result = apply(&mut input, EditCommand::MoveToStart);
        assert!(result.caret_moved && !result.value_changed);
        assert_eq!(input.caret(), 0);

        apply(&mut input, EditCommand::MoveRight);
        assert_eq!(input.caret(), 1);
        apply(&mut input, EditCommand::MoveToEnd);
        assert_eq!(input.caret(), 5);
        // Already at the end: nothing happens.
        assert!(apply(&mut input, EditCommand::MoveRight).did_nothing());
    }

    #[test]
    fn maxlength_caps_insertions() {
        let mut input = text_input(&[("maxlength", "3")]);
        type_text(&mut input, "abcdef");
        assert_eq!(input.control_value(), "abc");
    }

    #[test]
    fn number_ignores_maxlength_while_remaining_keyboard_editable() {
        let mut input = text_input(&[("type", "number"), ("maxlength", "2")]);
        type_text(&mut input, "1234");
        assert_eq!(input.control_value(), "1234");
        assert_eq!(input.caret(), 4);
    }

    #[test]
    fn readonly_and_disabled_refuse_edits_but_allow_the_caret() {
        for attribute in ["readonly", "disabled"] {
            let mut input = text_input(&[(attribute, ""), ("value", "fixed")]);
            assert!(apply(&mut input, EditCommand::Insert('x')).did_nothing());
            assert!(apply(&mut input, EditCommand::Backspace).did_nothing());
            assert_eq!(input.control_value(), "fixed");

            let moved = apply(&mut input, EditCommand::MoveToEnd);
            assert!(
                moved.caret_moved,
                "{attribute} should still allow the caret"
            );
        }
    }

    #[test]
    fn editing_starts_from_the_value_attribute() {
        let mut input = text_input(&[("value", "seed")]);
        apply(&mut input, EditCommand::MoveToEnd);
        type_text(&mut input, "ed");
        assert_eq!(input.control_value(), "seeded");
        // The attribute itself is untouched.
        assert_eq!(input.get_attr("value"), Some("seed"));
    }

    #[test]
    fn newlines_are_confined_to_textareas() {
        let mut input = text_input(&[]);
        assert!(apply(&mut input, EditCommand::InsertNewline).did_nothing());

        let mut area = textarea();
        type_text(&mut area, "a");
        apply(&mut area, EditCommand::InsertNewline);
        type_text(&mut area, "b");
        assert_eq!(area.control_value(), "a\nb");
    }

    #[test]
    fn vertical_movement_keeps_the_column() {
        let mut area = textarea();
        area.set_control_value("abcd\nef\nghij");

        area.set_caret(2); // line 0, column 2
        apply(&mut area, EditCommand::MoveDown);
        assert_eq!(
            caret_line_column(&area.control_value(), area.caret()),
            (1, 2)
        );

        // Line 1 is shorter, so moving down again clamps then keeps column 2.
        apply(&mut area, EditCommand::MoveDown);
        assert_eq!(
            caret_line_column(&area.control_value(), area.caret()),
            (2, 2)
        );

        apply(&mut area, EditCommand::MoveUp);
        assert_eq!(
            caret_line_column(&area.control_value(), area.caret()),
            (1, 2)
        );
    }

    #[test]
    fn home_and_end_work_per_line_in_a_textarea() {
        let mut area = textarea();
        area.set_control_value("first\nsecond");
        area.set_caret(8); // inside "second"

        apply(&mut area, EditCommand::MoveToStart);
        assert_eq!(area.caret(), 6);
        apply(&mut area, EditCommand::MoveToEnd);
        assert_eq!(area.caret(), 12);
    }

    #[test]
    fn multi_byte_text_is_edited_by_character() {
        let mut input = text_input(&[]);
        type_text(&mut input, "héllo");
        apply(&mut input, EditCommand::MoveToStart);
        apply(&mut input, EditCommand::MoveRight);
        apply(&mut input, EditCommand::Delete);
        assert_eq!(input.control_value(), "hllo");
    }

    #[test]
    fn clicking_picks_the_nearest_character_boundary() {
        let value = "hello world";
        assert_eq!(caret_for_offset(value, 16.0, 0.0), 0);
        assert_eq!(caret_for_offset(value, 16.0, -5.0), 0);

        let full = measure_text(value, 16.0);
        assert_eq!(caret_for_offset(value, 16.0, full + 50.0), value.len());

        // A click in the middle lands somewhere in the middle.
        let middle = caret_for_offset(value, 16.0, full / 2.0);
        assert!(middle > 0 && middle < value.len(), "got {middle}");
    }

    #[test]
    fn caret_line_column_maps_positions() {
        assert_eq!(caret_line_column("ab\ncd", 0), (0, 0));
        assert_eq!(caret_line_column("ab\ncd", 2), (0, 2));
        assert_eq!(caret_line_column("ab\ncd", 3), (1, 0));
        assert_eq!(caret_line_column("ab\ncd", 5), (1, 2));
        assert_eq!(value_lines("ab\ncd"), vec!["ab", "cd"]);
    }
}
