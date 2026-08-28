// ============================================================
//  platform.rs  —  minifb → engine input/cursor adapter
// ============================================================
//
//  The only place that knows about minifb's input and cursor representation.
//  It turns window events into engine [`KeyEvent`]s and translates the
//  backend-neutral cursor presentation plan into minifb's limited native
//  cursor vocabulary.
//
//  minifb reports characters and keys through two separate channels:
//  `get_keys_pressed` gives physical keys, and an input callback gives the
//  characters the platform's keyboard layout produced. Text entry uses the
//  latter, so layouts and Shift work without the adapter guessing.

use std::cell::RefCell;
use std::rc::Rc;

use browser_engine::cursor_presentation::{CursorPresentation, NativeCursor};
use browser_engine::input::{Key as EngineKey, KeyEvent, Modifiers};
use minifb::{CursorStyle, InputCallback, Key as MinifbKey, KeyRepeat, Window};

/// Characters the platform produced since the last frame.
#[derive(Default)]
struct CharacterQueue {
    characters: Vec<char>,
}

impl InputCallback for CharacterQueue {
    fn add_char(&mut self, uni_char: u32) {
        if let Some(character) = char::from_u32(uni_char) {
            // Control characters arrive as key presses instead.
            if !character.is_control() {
                self.characters.push(character);
            }
        }
    }
}

/// Collects window input and hands the engine a stream of key events.
pub struct InputAdapter {
    characters: Rc<RefCell<CharacterQueue>>,
}

impl InputAdapter {
    /// Attach to a window's character input.
    pub fn attach(window: &mut Window) -> InputAdapter {
        let characters = Rc::new(RefCell::new(CharacterQueue::default()));
        window.set_input_callback(Box::new(SharedQueue(characters.clone())));
        InputAdapter { characters }
    }

    /// Modifier keys currently held.
    pub fn modifiers(&self, window: &Window) -> Modifiers {
        Modifiers {
            shift: window.is_key_down(MinifbKey::LeftShift)
                || window.is_key_down(MinifbKey::RightShift),
            ctrl: window.is_key_down(MinifbKey::LeftCtrl)
                || window.is_key_down(MinifbKey::RightCtrl),
            alt: window.is_key_down(MinifbKey::LeftAlt) || window.is_key_down(MinifbKey::RightAlt),
        }
    }

    /// Every key event produced since the last call, in order: typed
    /// characters first, then the named keys pressed this frame.
    pub fn drain(&self, window: &Window) -> Vec<KeyEvent> {
        let modifiers = self.modifiers(window);
        let mut events: Vec<KeyEvent> = self
            .characters
            .borrow_mut()
            .characters
            .drain(..)
            .map(|c| KeyEvent::with_modifiers(EngineKey::Character(c), modifiers))
            .collect();

        for key in window.get_keys_pressed(KeyRepeat::Yes) {
            if let Some(engine_key) = translate(key) {
                events.push(KeyEvent::with_modifiers(engine_key, modifiers));
            }
        }
        events
    }
}

/// Apply the engine's backend-neutral cursor presentation to a minifb window.
///
/// `Hidden` implements CSS `cursor: none`; `SoftwareImage` hides minifb's
/// cursor so the decoded image can be painted by `cursor_overlay`; native
/// presentations keep the OS cursor visible and choose the nearest minifb
/// shape.
pub fn apply_cursor_presentation(window: &mut Window, presentation: CursorPresentation) {
    match presentation {
        CursorPresentation::Hidden | CursorPresentation::SoftwareImage => {
            window.set_cursor_visibility(false);
        }
        CursorPresentation::Native(cursor) => {
            window.set_cursor_visibility(true);
            window.set_cursor_style(cursor_style(cursor));
        }
    }
}

fn cursor_style(cursor: NativeCursor) -> CursorStyle {
    match cursor {
        NativeCursor::Arrow => CursorStyle::Arrow,
        NativeCursor::IBeam => CursorStyle::Ibeam,
        NativeCursor::Crosshair => CursorStyle::Crosshair,
        NativeCursor::ClosedHand => CursorStyle::ClosedHand,
        NativeCursor::OpenHand => CursorStyle::OpenHand,
        NativeCursor::ResizeLeftRight => CursorStyle::ResizeLeftRight,
        NativeCursor::ResizeUpDown => CursorStyle::ResizeUpDown,
        NativeCursor::ResizeAll => CursorStyle::ResizeAll,
    }
}

/// Wrapper so the queue can be shared between the callback and the adapter.
struct SharedQueue(Rc<RefCell<CharacterQueue>>);

impl InputCallback for SharedQueue {
    fn add_char(&mut self, uni_char: u32) {
        self.0.borrow_mut().add_char(uni_char);
    }
}

/// Map a physical key to the engine's key names.
///
/// Character keys are deliberately absent: they arrive through the character
/// callback, which already applied the keyboard layout.
fn translate(key: MinifbKey) -> Option<EngineKey> {
    Some(match key {
        MinifbKey::Enter | MinifbKey::NumPadEnter => EngineKey::Enter,
        MinifbKey::Tab => EngineKey::Tab,
        MinifbKey::Backspace => EngineKey::Backspace,
        MinifbKey::Delete => EngineKey::Delete,
        MinifbKey::Escape => EngineKey::Escape,
        MinifbKey::Left => EngineKey::ArrowLeft,
        MinifbKey::Right => EngineKey::ArrowRight,
        MinifbKey::Up => EngineKey::ArrowUp,
        MinifbKey::Down => EngineKey::ArrowDown,
        MinifbKey::Home => EngineKey::Home,
        MinifbKey::End => EngineKey::End,
        MinifbKey::PageUp => EngineKey::PageUp,
        MinifbKey::PageDown => EngineKey::PageDown,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_cursor_mapping_covers_every_backend_shape() {
        assert_eq!(cursor_style(NativeCursor::Arrow), CursorStyle::Arrow);
        assert_eq!(cursor_style(NativeCursor::IBeam), CursorStyle::Ibeam);
        assert_eq!(cursor_style(NativeCursor::Crosshair), CursorStyle::Crosshair);
        assert_eq!(cursor_style(NativeCursor::ClosedHand), CursorStyle::ClosedHand);
        assert_eq!(cursor_style(NativeCursor::OpenHand), CursorStyle::OpenHand);
        assert_eq!(
            cursor_style(NativeCursor::ResizeLeftRight),
            CursorStyle::ResizeLeftRight
        );
        assert_eq!(
            cursor_style(NativeCursor::ResizeUpDown),
            CursorStyle::ResizeUpDown
        );
        assert_eq!(cursor_style(NativeCursor::ResizeAll), CursorStyle::ResizeAll);
    }
}
