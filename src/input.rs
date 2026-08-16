// ============================================================
//  input.rs  —  Platform-independent input events
// ============================================================
//
//  The window backend (minifb, a test, anything else) translates its own key
//  representation into a [`KeyEvent`] and hands it to the browser. Nothing
//  below this line knows what a window is, and nothing above it knows what a
//  DOM is — which is what keeps keyboard handling out of `main.rs`.

use std::fmt;

/// A key, named the way the DOM names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    /// A key that produces text, carrying the character it produced.
    Character(char),
    Enter,
    Tab,
    Backspace,
    Delete,
    Escape,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    Home,
    End,
    PageUp,
    PageDown,
    /// Any key the adapter recognised but the engine has no behaviour for.
    Other(String),
}

impl Key {
    /// The value `event.key` reports.
    pub fn key_value(&self) -> String {
        match self {
            Key::Character(' ') => " ".to_string(),
            Key::Character(c) => c.to_string(),
            Key::Enter => "Enter".into(),
            Key::Tab => "Tab".into(),
            Key::Backspace => "Backspace".into(),
            Key::Delete => "Delete".into(),
            Key::Escape => "Escape".into(),
            Key::ArrowLeft => "ArrowLeft".into(),
            Key::ArrowRight => "ArrowRight".into(),
            Key::ArrowUp => "ArrowUp".into(),
            Key::ArrowDown => "ArrowDown".into(),
            Key::Home => "Home".into(),
            Key::End => "End".into(),
            Key::PageUp => "PageUp".into(),
            Key::PageDown => "PageDown".into(),
            Key::Other(name) => name.clone(),
        }
    }

    /// A simplified `event.code`: the physical key name.
    ///
    /// Real browsers report the key's position on the keyboard; this reports
    /// the obvious code for letters, digits and named keys, which is enough
    /// for scripts that check for `Space`, `KeyA` or `Enter`.
    pub fn code_value(&self) -> String {
        match self {
            Key::Character(' ') => "Space".into(),
            Key::Character(c) if c.is_ascii_alphabetic() => {
                format!("Key{}", c.to_ascii_uppercase())
            }
            Key::Character(c) if c.is_ascii_digit() => format!("Digit{c}"),
            Key::Character(_) => "Unidentified".into(),
            other => other.key_value(),
        }
    }

    /// The character this key inserts into a text field, if any.
    pub fn printable(&self) -> Option<char> {
        match self {
            Key::Character(c) => Some(*c),
            _ => None,
        }
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.key_value())
    }
}

/// Modifier keys held down when an event was produced.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

impl Modifiers {
    pub const NONE: Modifiers = Modifiers {
        shift: false,
        ctrl: false,
        alt: false,
    };

    pub fn shift() -> Modifiers {
        Modifiers {
            shift: true,
            ..Modifiers::NONE
        }
    }

    pub fn ctrl() -> Modifiers {
        Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        }
    }

    /// True when a modifier that should suppress plain text entry is held.
    pub fn has_command_modifier(&self) -> bool {
        self.ctrl || self.alt
    }
}

/// One keyboard event on its way into the DOM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub modifiers: Modifiers,
}

impl KeyEvent {
    pub fn new(key: Key) -> KeyEvent {
        KeyEvent {
            key,
            modifiers: Modifiers::NONE,
        }
    }

    pub fn with_modifiers(key: Key, modifiers: Modifiers) -> KeyEvent {
        KeyEvent { key, modifiers }
    }

    /// A printable character typed with no command modifier held.
    pub fn typed_character(&self) -> Option<char> {
        if self.modifiers.has_command_modifier() {
            return None;
        }
        self.key.printable()
    }

    /// Convenience for tests and adapters: a plain character keypress.
    pub fn character(c: char) -> KeyEvent {
        KeyEvent::new(Key::Character(c))
    }
}

/// Which direction focus should move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusDirection {
    Forward,
    Backward,
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_values_match_the_dom_names() {
        assert_eq!(Key::Character('a').key_value(), "a");
        assert_eq!(Key::Character(' ').key_value(), " ");
        assert_eq!(Key::Enter.key_value(), "Enter");
        assert_eq!(Key::ArrowLeft.key_value(), "ArrowLeft");
    }

    #[test]
    fn codes_describe_the_physical_key() {
        assert_eq!(Key::Character('a').code_value(), "KeyA");
        assert_eq!(Key::Character('Z').code_value(), "KeyZ");
        assert_eq!(Key::Character('7').code_value(), "Digit7");
        assert_eq!(Key::Character(' ').code_value(), "Space");
        assert_eq!(Key::Backspace.code_value(), "Backspace");
    }

    #[test]
    fn command_modifiers_suppress_text_entry() {
        assert_eq!(KeyEvent::character('x').typed_character(), Some('x'));
        let with_ctrl = KeyEvent::with_modifiers(Key::Character('x'), Modifiers::ctrl());
        assert_eq!(with_ctrl.typed_character(), None);
        // Shift is part of producing the character, so it does not suppress it.
        let with_shift = KeyEvent::with_modifiers(Key::Character('X'), Modifiers::shift());
        assert_eq!(with_shift.typed_character(), Some('X'));
    }

    #[test]
    fn non_printable_keys_have_no_character() {
        assert_eq!(KeyEvent::new(Key::Enter).typed_character(), None);
        assert_eq!(KeyEvent::new(Key::Tab).typed_character(), None);
    }
}
