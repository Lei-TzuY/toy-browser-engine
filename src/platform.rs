// ============================================================
//  platform.rs  —  minifb → engine input adapter
// ============================================================
//
//  The only place that knows about minifb's key representation. It turns
//  window events into the engine's [`KeyEvent`]s, so `main` never interprets
//  keystrokes and the engine never mentions a window toolkit.
//
//  minifb reports characters and keys through two separate channels:
//  `get_keys_pressed` gives physical keys, and an input callback gives the
//  characters the platform's keyboard layout produced. Text entry uses the
//  latter, so layouts and Shift work without the adapter guessing.

use std::cell::RefCell;
use std::rc::Rc;

use browser_engine::input::{Key as EngineKey, KeyEvent, Modifiers};
use minifb::{InputCallback, Key as MinifbKey, KeyRepeat, Window};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::tabs::{ShortcutKey, ShortcutKeyFilter};

/// Characters the platform produced since the last frame.
#[derive(Default)]
struct CharacterQueue {
    characters: Vec<KeyEvent>,
    shortcut_keys: ShortcutKeyFilter,
    modifier_keys: [bool; 6],
    // minifb 0.28 X11/macOS call set_key_state before add_char. Wayland
    // calls them in the opposite order; hold its text until the key callback.
    text_first: bool,
    pending_text: Vec<u32>,
    // Snapshot at key-down, rather than looking at ownership when draining:
    // a final character can be queued before release but drained after it.
    text_owned: bool,
}

impl CharacterQueue {
    fn modifiers(&self) -> Modifiers {
        Modifiers {
            ctrl: self.modifier_keys[0] || self.modifier_keys[1],
            shift: self.modifier_keys[2] || self.modifier_keys[3],
            alt: self.modifier_keys[4] || self.modifier_keys[5],
        }
    }

    fn push_char(&mut self, uni_char: u32) {
        if self.text_owned {
            return;
        }
        if let Some(character) = char::from_u32(uni_char) {
            if !character.is_control() {
                self.characters.push(KeyEvent::with_modifiers(
                    EngineKey::Character(character),
                    self.modifiers(),
                ));
            }
        }
    }

    fn drain(&mut self, pressed: Vec<MinifbKey>, modifiers: Modifiers) -> Vec<KeyEvent> {
        // Unassociated text (e.g. an IME commit) is ordinary text. Never infer
        // its physical origin from the character or from another held key.
        self.text_owned = false;
        for character in std::mem::take(&mut self.pending_text) {
            self.push_char(character);
        }
        let mut events = std::mem::take(&mut self.characters);
        for key in pressed {
            if shortcut_key(key).is_some_and(|key| self.shortcut_keys.owns(key)) {
                continue;
            }
            if let Some(engine_key) = translate(key) {
                events.push(KeyEvent::with_modifiers(engine_key, modifiers));
            }
        }
        events
    }
}

impl InputCallback for CharacterQueue {
    fn add_char(&mut self, uni_char: u32) {
        if self.text_first {
            self.pending_text.push(uni_char);
        } else {
            self.push_char(uni_char);
        }
    }

    fn set_key_state(&mut self, key: MinifbKey, down: bool) {
        let modifier = match key {
            MinifbKey::LeftCtrl => Some(0),
            MinifbKey::RightCtrl => Some(1),
            MinifbKey::LeftShift => Some(2),
            MinifbKey::RightShift => Some(3),
            MinifbKey::LeftAlt => Some(4),
            MinifbKey::RightAlt => Some(5),
            _ => None,
        };
        if let Some(index) = modifier {
            self.modifier_keys[index] = down;
        }
        let modifiers = self.modifiers();
        let shortcut = shortcut_key(key);
        if let Some(shortcut) = shortcut {
            self.shortcut_keys
                .key_state(shortcut, down, modifiers.ctrl, modifiers.alt);
        }
        // End the previous callback's text group. In particular, a later
        // IME/unmapped character must not inherit a released key's owner.
        self.text_owned = false;
        if down && modifier.is_none() {
            // Associate text with the producing physical key, not its Unicode
            // value. Shift+1 and a layout's unrelated '1' stay distinct.
            let mut pending = std::mem::take(&mut self.pending_text);
            // Wayland handle_key emits one codepoint before set_key_state.
            // Earlier text without a key callback has no physical source.
            let associated = pending.pop();
            for character in pending {
                self.push_char(character);
            }
            self.text_owned = shortcut.is_some_and(|key| self.shortcut_keys.owns(key));
            if let Some(character) = associated {
                self.push_char(character);
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
        let text_first = window
            .window_handle()
            .is_ok_and(|handle| matches!(handle.as_raw(), RawWindowHandle::Wayland(_)));
        let characters = Rc::new(RefCell::new(CharacterQueue {
            text_first,
            ..CharacterQueue::default()
        }));
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
        self.characters.borrow_mut().drain(
            window.get_keys_pressed(KeyRepeat::Yes),
            self.modifiers(window),
        )
    }
}

/// Wrapper so the queue can be shared between the callback and the adapter.
struct SharedQueue(Rc<RefCell<CharacterQueue>>);

impl InputCallback for SharedQueue {
    fn add_char(&mut self, uni_char: u32) {
        self.0.borrow_mut().add_char(uni_char);
    }

    fn set_key_state(&mut self, key: MinifbKey, down: bool) {
        self.0.borrow_mut().set_key_state(key, down);
    }
}

fn shortcut_key(key: MinifbKey) -> Option<ShortcutKey> {
    Some(match key {
        MinifbKey::Tab => ShortcutKey::Tab,
        MinifbKey::T => ShortcutKey::T,
        MinifbKey::W => ShortcutKey::W,
        MinifbKey::Key1 => ShortcutKey::Key1,
        MinifbKey::Key2 => ShortcutKey::Key2,
        MinifbKey::Key3 => ShortcutKey::Key3,
        MinifbKey::Key4 => ShortcutKey::Key4,
        MinifbKey::Key5 => ShortcutKey::Key5,
        MinifbKey::Key6 => ShortcutKey::Key6,
        MinifbKey::Key7 => ShortcutKey::Key7,
        MinifbKey::Key8 => ShortcutKey::Key8,
        MinifbKey::Key9 => ShortcutKey::Key9,
        _ => return None,
    })
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
    use crate::tabs::{Applied, TabCommand, Tabs};
    use browser_engine::{
        browser::Browser,
        net::{MemoryLoader, Url},
        script::dom_api,
    };

    fn queue(text_first: bool) -> CharacterQueue {
        CharacterQueue {
            text_first,
            ..CharacterQueue::default()
        }
    }

    // Exercise minifb's callback contract directly, including a full press
    // and release between drains. No sleeps or elapsed-frame assumptions.
    fn press(queue: &mut CharacterQueue, key: MinifbKey, character: char) {
        if queue.text_first {
            queue.add_char(character as u32);
        }
        queue.set_key_state(key, true);
        if !queue.text_first {
            queue.add_char(character as u32);
        }
    }

    fn drain(queue: &mut CharacterQueue, keys: Vec<MinifbKey>) -> Vec<KeyEvent> {
        queue.drain(keys, queue.modifiers())
    }

    fn text(events: &[KeyEvent]) -> String {
        events.iter().filter_map(|e| e.key.printable()).collect()
    }

    fn type_aw1b(queue: &mut CharacterQueue) {
        for (key, ch) in [
            (MinifbKey::A, 'a'),
            (MinifbKey::W, 'w'),
            (MinifbKey::Key1, '1'),
            (MinifbKey::B, 'b'),
        ] {
            press(queue, key, ch);
            queue.set_key_state(key, false);
        }
    }

    #[test]
    fn r1_ctrl_t_long_frame_preserves_exact_aw1b_callback_batch() {
        for text_first in [false, true] {
            let mut queue = queue(text_first);
            queue.set_key_state(MinifbKey::LeftCtrl, true);
            press(&mut queue, MinifbKey::T, '\u{14}');
            assert!(drain(&mut queue, vec![]).is_empty()); // Ctrl+T starts load
                                                           // These callbacks all arrive in ONE drain after the synchronous
                                                           // load. W and 1 were never shortcut keys; T belongs to chrome.
            queue.set_key_state(MinifbKey::LeftCtrl, false);
            queue.set_key_state(MinifbKey::T, false);
            type_aw1b(&mut queue);
            let events = drain(&mut queue, vec![]);
            assert_eq!(text(&events), "aw1b"); // old filter returned "ab"
            assert!(events.iter().all(|e| e.modifiers == Modifiers::NONE));
        }
    }

    #[test]
    fn r2_tab_remains_owned_for_four_ctrl_up_frames_and_final_repeat() {
        for text_first in [false, true] {
            let mut queue = queue(text_first);
            queue.set_key_state(MinifbKey::LeftCtrl, true);
            queue.set_key_state(MinifbKey::LeftShift, true);
            press(&mut queue, MinifbKey::Tab, '\t');
            assert!(drain(&mut queue, vec![MinifbKey::Tab]).is_empty());
            queue.set_key_state(MinifbKey::LeftCtrl, false);
            for frame in 0..4 {
                if frame == 2 {
                    queue.set_key_state(MinifbKey::LeftShift, false);
                }
                press(&mut queue, MinifbKey::Tab, '\t');
                assert!(
                    drain(&mut queue, vec![MinifbKey::Tab]).is_empty(),
                    "frame {frame}"
                );
            }
            // Last repeat followed by release BEFORE the next drain.
            press(&mut queue, MinifbKey::Tab, '\t');
            queue.set_key_state(MinifbKey::Tab, false);
            assert!(drain(&mut queue, vec![]).is_empty());
            press(&mut queue, MinifbKey::Tab, '\t');
            assert_eq!(
                drain(&mut queue, vec![MinifbKey::Tab]),
                vec![KeyEvent::new(EngineKey::Tab)]
            );
        }
    }

    #[test]
    fn owned_letters_and_all_digits_drop_repeats_but_new_presses_pass() {
        for text_first in [false, true] {
            for (key, ch) in [
                (MinifbKey::T, 't'),
                (MinifbKey::W, 'w'),
                (MinifbKey::Key1, '1'),
                (MinifbKey::Key2, '2'),
                (MinifbKey::Key3, '3'),
                (MinifbKey::Key4, '4'),
                (MinifbKey::Key5, '5'),
                (MinifbKey::Key6, '6'),
                (MinifbKey::Key7, '7'),
                (MinifbKey::Key8, '8'),
                (MinifbKey::Key9, '9'),
            ] {
                let mut queue = queue(text_first);
                queue.set_key_state(MinifbKey::RightCtrl, true);
                press(&mut queue, key, ch);
                assert!(drain(&mut queue, vec![key]).is_empty());
                queue.set_key_state(MinifbKey::RightCtrl, false);
                for _ in 0..4 {
                    press(&mut queue, key, ch);
                    assert!(drain(&mut queue, vec![key]).is_empty());
                }
                // Release, fresh press of SAME key, and ordinary keys all
                // queued before one drain: a held-key-only filter fails here.
                press(&mut queue, key, ch);
                queue.set_key_state(key, false);
                press(&mut queue, key, ch);
                queue.set_key_state(key, false);
                type_aw1b(&mut queue);
                assert_eq!(text(&drain(&mut queue, vec![])), format!("{ch}aw1b"));
            }
        }
    }

    #[test]
    fn text_ownership_uses_physical_source_instead_of_layout_character() {
        for text_first in [false, true] {
            let mut queue = queue(text_first);
            queue.set_key_state(MinifbKey::LeftCtrl, true);
            press(&mut queue, MinifbKey::Key1, '1');
            queue.set_key_state(MinifbKey::LeftCtrl, false);
            queue.set_key_state(MinifbKey::LeftShift, true);
            press(&mut queue, MinifbKey::Key1, '!'); // owned physical digit
            queue.set_key_state(MinifbKey::LeftShift, false);
            // A different layout/key produces '1' while physical 1 is owned.
            press(&mut queue, MinifbKey::Q, '1');
            queue.set_key_state(MinifbKey::Q, false);
            press(&mut queue, MinifbKey::NumPad1, '1');
            queue.set_key_state(MinifbKey::NumPad1, false);
            assert_eq!(text(&drain(&mut queue, vec![MinifbKey::Key1])), "11");
        }
    }

    #[test]
    fn unmapped_or_ime_text_does_not_inherit_another_keys_owner() {
        for text_first in [false, true] {
            let mut queue = queue(text_first);
            queue.set_key_state(MinifbKey::LeftCtrl, true);
            press(&mut queue, MinifbKey::W, '\u{17}');
            queue.set_key_state(MinifbKey::W, false);
            queue.set_key_state(MinifbKey::LeftCtrl, false);
            queue.add_char('字' as u32); // no corresponding physical callback
            queue.set_key_state(MinifbKey::LeftCtrl, true);
            press(&mut queue, MinifbKey::Key1, '1');
            assert_eq!(text(&drain(&mut queue, vec![])), "字");
        }
    }

    #[test]
    fn altgr_and_unrelated_ctrl_keys_do_not_acquire_shortcut_ownership() {
        for text_first in [false, true] {
            let mut queue = queue(text_first);
            queue.set_key_state(MinifbKey::LeftCtrl, true);
            queue.set_key_state(MinifbKey::RightAlt, true);
            press(&mut queue, MinifbKey::W, 'w');
            assert_eq!(text(&drain(&mut queue, vec![])), "w");
            queue.set_key_state(MinifbKey::W, false);
            queue.set_key_state(MinifbKey::RightAlt, false);
            press(&mut queue, MinifbKey::L, '\u{c}');
            queue.set_key_state(MinifbKey::L, false);
            queue.set_key_state(MinifbKey::LeftCtrl, false);
            type_aw1b(&mut queue);
            assert_eq!(text(&drain(&mut queue, vec![])), "aw1b");
        }
    }

    fn browser() -> Browser {
        let mut loader = MemoryLoader::new();
        loader.insert(
            "demo:///keys",
            r#"<html><body><p id="log">empty</p><script>
            document.body.addEventListener("keydown", function(e) {
                document.getElementById("log").textContent = e.key;
            });</script></body></html>"#,
        );
        Browser::open(Box::new(loader), &Url::parse("demo:///keys").unwrap()).unwrap()
    }

    #[test]
    fn filtered_events_stay_in_focused_address_editor_then_reach_only_active_dom() {
        let mut tabs = Tabs::new(browser(), 800);
        tabs.active_mut().toolbar.focus_address("old draft");
        let mut queue = queue(false);
        queue.set_key_state(MinifbKey::LeftCtrl, true);
        press(&mut queue, MinifbKey::T, '\u{14}');
        assert_eq!(
            tabs.apply(TabCommand::New, 10, || Ok(browser())),
            Applied::Continue
        );
        queue.set_key_state(MinifbKey::LeftCtrl, false);
        queue.set_key_state(MinifbKey::T, false);
        type_aw1b(&mut queue);
        tabs.active_mut()
            .handle_keys(drain(&mut queue, vec![]), false);
        assert_eq!(tabs.active().toolbar.editor().unwrap().value(), "aw1b");
        assert_eq!(
            tabs.get(0).unwrap().toolbar.editor().unwrap().value(),
            "old draft"
        );
        for tab in tabs.iter_mut() {
            assert!(dom_api::outer_html(&tab.browser.document().dom).contains(">empty</p>"));
        }
        tabs.active_mut().toolbar.cancel();
        press(&mut queue, MinifbKey::W, 'w');
        tabs.active_mut()
            .handle_keys(drain(&mut queue, vec![]), false);
        assert!(dom_api::outer_html(&tabs.active().browser.document().dom).contains(">w</p>"));
        assert!(
            dom_api::outer_html(&tabs.get(0).unwrap().browser.document().dom)
                .contains(">empty</p>")
        );
    }
}
