// ============================================================
// location_input.rs — single-line address editor for --window
// ============================================================
//
//  Toolkit-neutral editing state behind the toolbar's address field: the
//  text, a caret and a selection anchor, all counted in `char`s so multibyte
//  input never splits a code point. The toolbar decides when keys reach it
//  and draws it; this file only edits.

use browser_engine::input::{Key, KeyEvent};

pub(crate) struct LocationInput {
    value: String,
    /// Caret position, in chars from the start.
    caret: usize,
    /// The other end of the selection. Equal to `caret` when nothing is
    /// selected.
    anchor: usize,
}

pub(crate) enum LocationAction {
    Continue,
    Cancel,
    Go(String),
}

const MAX_LOCATION_CHARS: usize = 2048;

impl LocationInput {
    /// Start editing `current_url` with all of it selected, like a familiar
    /// address field: the first keystroke replaces it.
    pub(crate) fn new(current_url: String) -> Self {
        let len = current_url.chars().count();
        Self {
            value: current_url,
            caret: len,
            anchor: 0,
        }
    }

    pub(crate) fn value(&self) -> &str {
        &self.value
    }

    pub(crate) fn caret(&self) -> usize {
        self.caret
    }

    /// The selected char range, if any, as `(start, end)`.
    pub(crate) fn selection(&self) -> Option<(usize, usize)> {
        (self.anchor != self.caret)
            .then(|| (self.anchor.min(self.caret), self.anchor.max(self.caret)))
    }

    pub(crate) fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.len();
    }

    /// Move the caret to `index` (clamped). With `extend` the selection
    /// anchor stays put, as for Shift+click or a drag.
    pub(crate) fn place_caret(&mut self, index: usize, extend: bool) {
        self.caret = index.min(self.len());
        if !extend {
            self.anchor = self.caret;
        }
    }

    pub(crate) fn handle(&mut self, event: &KeyEvent) -> LocationAction {
        let shift = event.modifiers.shift;
        let command = event.modifiers.has_command_modifier();
        match &event.key {
            Key::Escape => return LocationAction::Cancel,
            Key::Enter => {
                let target = self.value.trim();
                return if target.is_empty() {
                    LocationAction::Cancel
                } else {
                    LocationAction::Go(target.to_string())
                };
            }
            Key::Backspace | Key::Delete if !command => {
                if !self.delete_selection() {
                    let len = self.len();
                    let range = match event.key {
                        Key::Backspace if self.caret > 0 => Some(self.caret - 1..self.caret),
                        Key::Delete if self.caret < len => Some(self.caret..self.caret + 1),
                        _ => None,
                    };
                    if let Some(range) = range {
                        self.caret = range.start;
                        self.anchor = range.start;
                        self.remove_chars(range.start, range.end);
                    }
                }
            }
            Key::ArrowLeft => {
                let target = match self.selection() {
                    Some((start, _)) if !shift => start,
                    _ => self.caret.saturating_sub(1),
                };
                self.place_caret(target, shift);
            }
            Key::ArrowRight => {
                let target = match self.selection() {
                    Some((_, end)) if !shift => end,
                    _ => self.caret + 1,
                };
                self.place_caret(target, shift);
            }
            Key::Home => self.place_caret(0, shift),
            Key::End => self.place_caret(self.len(), shift),
            Key::Character('a' | 'A') if event.modifiers.ctrl && !event.modifiers.alt => {
                self.select_all();
            }
            Key::Character(character) if !command && !character.is_control() => {
                self.insert(*character);
            }
            _ => {}
        }
        LocationAction::Continue
    }

    fn len(&self) -> usize {
        self.value.chars().count()
    }

    /// Replace the selection (if any) with `character` at the caret.
    fn insert(&mut self, character: char) {
        self.delete_selection();
        if self.len() >= MAX_LOCATION_CHARS {
            return;
        }
        let at = self.byte_offset(self.caret);
        self.value.insert(at, character);
        self.caret += 1;
        self.anchor = self.caret;
    }

    /// Remove the selected text. Returns false when nothing was selected.
    fn delete_selection(&mut self) -> bool {
        let Some((start, end)) = self.selection() else {
            return false;
        };
        self.remove_chars(start, end);
        self.caret = start;
        self.anchor = start;
        true
    }

    fn remove_chars(&mut self, start: usize, end: usize) {
        let (from, to) = (self.byte_offset(start), self.byte_offset(end));
        self.value.replace_range(from..to, "");
    }

    fn byte_offset(&self, chars: usize) -> usize {
        self.value
            .char_indices()
            .nth(chars)
            .map_or(self.value.len(), |(offset, _)| offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use browser_engine::input::{KeyEvent, Modifiers};

    fn key(key: Key) -> KeyEvent {
        KeyEvent::with_modifiers(key, Modifiers::NONE)
    }

    fn shifted(key: Key) -> KeyEvent {
        KeyEvent::with_modifiers(key, Modifiers::shift())
    }

    fn type_text(editor: &mut LocationInput, text: &str) {
        for character in text.chars() {
            editor.handle(&key(Key::Character(character)));
        }
    }

    #[test]
    fn new_editor_selects_the_whole_url() {
        let editor = LocationInput::new("http://old.test/a".into());
        assert_eq!(editor.selection(), Some((0, 17)));
        assert_eq!(editor.caret(), 17);
    }

    #[test]
    fn first_printable_character_replaces_selected_url() {
        let mut editor = LocationInput::new("http://old.test/a".into());
        type_text(&mut editor, "hi");
        assert_eq!(editor.value(), "hi");
        assert_eq!(editor.selection(), None);
    }

    #[test]
    fn enter_without_typing_uses_selected_url() {
        let mut editor = LocationInput::new("http://site.test/path".into());
        let LocationAction::Go(url) = editor.handle(&key(Key::Enter)) else {
            panic!("Enter must navigate");
        };
        assert_eq!(url, "http://site.test/path");
    }

    #[test]
    fn backspace_clears_selection_then_removes_one_unicode_character() {
        let mut editor = LocationInput::new("http://old.test".into());
        editor.handle(&key(Key::Backspace));
        assert!(editor.value().is_empty());
        editor.handle(&key(Key::Character('中')));
        editor.handle(&key(Key::Character('文')));
        editor.handle(&key(Key::Backspace));
        assert_eq!(editor.value(), "中");
    }

    #[test]
    fn escape_cancels_and_empty_enter_does_not_navigate() {
        let mut editor = LocationInput::new("http://old.test".into());
        assert!(matches!(
            editor.handle(&key(Key::Escape)),
            LocationAction::Cancel
        ));
        editor.handle(&key(Key::Backspace));
        assert!(matches!(
            editor.handle(&key(Key::Enter)),
            LocationAction::Cancel
        ));
    }

    #[test]
    fn command_modified_text_does_not_enter_address() {
        let mut editor = LocationInput::new("http://old.test".into());
        let ctrl_l = KeyEvent::with_modifiers(Key::Character('l'), Modifiers::ctrl());
        editor.handle(&ctrl_l);
        assert_eq!(editor.value(), "http://old.test");
        assert_eq!(editor.selection(), Some((0, 15)));
    }

    #[test]
    fn address_input_is_bounded_and_control_characters_are_ignored() {
        let mut editor = LocationInput::new(String::new());
        for _ in 0..(MAX_LOCATION_CHARS + 10) {
            editor.handle(&key(Key::Character('x')));
        }
        editor.handle(&key(Key::Character('\n')));
        assert_eq!(editor.value().chars().count(), MAX_LOCATION_CHARS);
    }

    #[test]
    fn arrows_collapse_selection_and_insert_in_the_middle() {
        let mut editor = LocationInput::new("http://a.test/".into());
        editor.handle(&key(Key::ArrowLeft));
        assert_eq!((editor.caret(), editor.selection()), (0, None));
        editor.handle(&key(Key::End));
        editor.handle(&key(Key::ArrowLeft));
        type_text(&mut editor, "b");
        assert_eq!(editor.value(), "http://a.testb/");
        editor.handle(&key(Key::Home));
        editor.handle(&key(Key::Delete));
        assert_eq!(editor.value(), "ttp://a.testb/");
    }

    #[test]
    fn shift_arrows_extend_a_selection_that_typing_replaces() {
        let mut editor = LocationInput::new("http://a.test/x".into());
        editor.handle(&key(Key::End));
        editor.handle(&shifted(Key::ArrowLeft));
        editor.handle(&shifted(Key::ArrowLeft));
        assert_eq!(editor.selection(), Some((13, 15)));
        type_text(&mut editor, "yz");
        assert_eq!(editor.value(), "http://a.testyz");
        editor.handle(&shifted(Key::Home));
        assert_eq!(editor.selection(), Some((0, 15)));
    }

    #[test]
    fn ctrl_a_selects_everything_and_caret_placement_clamps() {
        let mut editor = LocationInput::new("héllo".into());
        editor.place_caret(99, false);
        assert_eq!(editor.caret(), 5);
        editor.place_caret(1, false);
        editor.place_caret(3, true);
        assert_eq!(editor.selection(), Some((1, 3)));
        editor.handle(&key(Key::Backspace));
        assert_eq!(editor.value(), "hlo");
        editor.handle(&KeyEvent::with_modifiers(
            Key::Character('a'),
            Modifiers::ctrl(),
        ));
        assert_eq!(editor.selection(), Some((0, 3)));
    }

    #[test]
    fn delete_and_backspace_at_the_edges_are_harmless() {
        let mut editor = LocationInput::new("ab".into());
        editor.handle(&key(Key::End));
        editor.handle(&key(Key::Delete));
        editor.handle(&key(Key::Home));
        editor.handle(&key(Key::Backspace));
        assert_eq!(editor.value(), "ab");
    }
}
