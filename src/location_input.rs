// ============================================================
// location_input.rs — minimal toolkit-neutral URL entry for --window
// ============================================================

use browser_engine::input::{Key, KeyEvent};

/// The minifb title bar displays an editable URL while Ctrl+L is active.
/// This is an intentionally small UI entry point, not a replacement for a
/// conventional on-canvas toolbar.
pub(crate) struct LocationInput {
    value: String,
    selected: bool,
}

pub(crate) enum LocationAction {
    Continue,
    Cancel,
    Go(String),
}

const MAX_LOCATION_CHARS: usize = 2048;

impl LocationInput {
    pub(crate) fn new(current_url: String) -> Self {
        Self {
            value: current_url,
            selected: true,
        }
    }

    pub(crate) fn title(&self) -> String {
        if self.selected {
            format!("Address [selected]: {} — type / Enter / Esc", self.value)
        } else {
            format!("Address: {} — Enter to open / Esc to cancel", self.value)
        }
    }

    pub(crate) fn handle(&mut self, event: &KeyEvent) -> LocationAction {
        match &event.key {
            Key::Escape => LocationAction::Cancel,
            Key::Enter => {
                let target = self.value.trim();
                if target.is_empty() {
                    LocationAction::Cancel
                } else {
                    LocationAction::Go(target.to_string())
                }
            }
            Key::Backspace | Key::Delete if !event.modifiers.has_command_modifier() => {
                if self.selected {
                    self.value.clear();
                } else {
                    self.value.pop();
                }
                self.selected = false;
                LocationAction::Continue
            }
            Key::Character(character)
                if !event.modifiers.has_command_modifier() && !character.is_control() =>
            {
                if self.selected {
                    self.value.clear();
                    self.selected = false;
                }
                if self.value.chars().count() < MAX_LOCATION_CHARS {
                    self.value.push(*character);
                }
                LocationAction::Continue
            }
            _ => LocationAction::Continue,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use browser_engine::input::{KeyEvent, Modifiers};

    fn key(key: Key) -> KeyEvent {
        KeyEvent::with_modifiers(key, Modifiers::NONE)
    }

    #[test]
    fn first_printable_character_replaces_selected_url() {
        let mut editor = LocationInput::new("http://old.test/a".into());
        editor.handle(&key(Key::Character('h')));
        editor.handle(&key(Key::Character('i')));
        assert_eq!(editor.value, "hi");
        assert!(!editor.selected);
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
        assert!(editor.value.is_empty());
        editor.handle(&key(Key::Character('中')));
        editor.handle(&key(Key::Character('文')));
        editor.handle(&key(Key::Backspace));
        assert_eq!(editor.value, "中");
    }

    #[test]
    fn escape_cancels_and_empty_enter_does_not_navigate() {
        let mut editor = LocationInput::new("http://old.test".into());
        assert!(matches!(editor.handle(&key(Key::Escape)), LocationAction::Cancel));
        editor.handle(&key(Key::Backspace));
        assert!(matches!(editor.handle(&key(Key::Enter)), LocationAction::Cancel));
    }

    #[test]
    fn command_modified_text_does_not_enter_address() {
        let mut editor = LocationInput::new("http://old.test".into());
        let ctrl_l = KeyEvent::with_modifiers(Key::Character('l'), Modifiers::ctrl());
        editor.handle(&ctrl_l);
        assert_eq!(editor.value, "http://old.test");
        assert!(editor.selected);
    }

    #[test]
    fn address_input_is_bounded_and_control_characters_are_ignored() {
        let mut editor = LocationInput::new(String::new());
        editor.handle(&key(Key::Character('x')));
        for _ in 0..(MAX_LOCATION_CHARS + 10) {
            editor.handle(&key(Key::Character('x')));
        }
        editor.handle(&key(Key::Character('\n')));
        assert_eq!(editor.value.chars().count(), MAX_LOCATION_CHARS);
    }
}
