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

use crate::tabs::{ShortcutKey, ShortcutKeyFilter};

/// A keyboard-triggered tab shortcut captured at its exact callback instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TabShortcut {
    New,
    CloseActive,
    Next,
    Previous,
    Number(usize),
}

/// A browser-chrome shortcut captured at its exact callback instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChromeShortcut {
    FocusAddress,
    Back,
    Forward,
    Reload,
    ScrollDown,
    ScrollUp,
}

/// One chronological keyboard, shortcut or IME action from a frame's event pump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FrameAction {
    Tab(TabShortcut),
    Chrome(ChromeShortcut),
    Key(KeyEvent),
    ImePreedit { text: String, cursor: usize },
}

/// Callback-time text, shortcuts and ownership. Native backends supply each
/// character's source explicitly; `add_char` alone means unassociated/IME text.
/// Shortcuts are resolved at the exact callback instant rather than by pairing
/// end-of-frame modifier state with E3-latched `is_key_pressed`, so a key
/// pressed and released *before* `Ctrl`/`Alt` goes down can never become a
/// shortcut in the same frame.
#[derive(Default)]
struct CharacterQueue {
    actions: Vec<FrameAction>,
    /// Physical keys whose initial press already ran through the callback in
    /// this frame, so `drain_actions` does not double-emit or re-evaluate them
    /// under a later modifier state.
    callback_pressed_keys: Vec<MinifbKey>,
    down_codes: std::collections::BTreeSet<u32>,
    shortcut_keys: ShortcutKeyFilter,
    modifier_keys: [bool; 6],
    /// Physical key codes that produced an AltGr character on a text-first
    /// backend while `RightAlt` was held, suppressing tentative `Reload`.
    altgr_text_codes: std::collections::BTreeSet<u32>,
    /// Tentative `ChromeShortcut::Reload` emitted on key-first `RightAlt+R`,
    /// cancelled if an AltGr character arrives for the same physical code.
    pending_right_alt_r: Option<(u32, usize)>,
    ime_activity: bool,
}

impl CharacterQueue {
    fn modifiers(&self) -> Modifiers {
        Modifiers {
            ctrl: self.modifier_keys[0] || self.modifier_keys[1],
            shift: self.modifier_keys[2] || self.modifier_keys[3],
            alt: self.modifier_keys[4] || self.modifier_keys[5],
        }
    }

    fn push_char(&mut self, uni_char: u32, clear_command_modifiers: bool) {
        if let Some(character) = char::from_u32(uni_char) {
            if !character.is_control() {
                let mut modifiers = self.modifiers();
                // AltGr produces text (often reported as LCtrl+RightAlt), and
                // unassociated/IME commits are already finalized text; neither
                // should inherit held Ctrl/Alt to trigger shortcuts or drop
                // characters while Ctrl/Alt is held.
                if self.modifier_keys[5] || clear_command_modifiers {
                    modifiers.ctrl = false;
                    modifiers.alt = false;
                }
                self.actions.push(FrameAction::Key(KeyEvent::with_modifiers(
                    EngineKey::Character(character),
                    modifiers,
                )));
            }
        }
    }

    /// Drain all chronological actions captured during callbacks, followed by
    /// any genuine held-key auto-repeats reported by minifb for keys that are
    /// still physically down.
    fn drain_actions(
        &mut self,
        pressed: Vec<MinifbKey>,
        still_down: impl Fn(MinifbKey) -> bool,
        left_alt: bool,
        right_alt: bool,
        ctrl: bool,
        shift: bool,
    ) -> Vec<FrameAction> {
        let mut actions = std::mem::take(&mut self.actions);
        let mut callback_pressed = std::mem::take(&mut self.callback_pressed_keys);
        self.pending_right_alt_r = None;
        let any_alt = left_alt || right_alt;
        let modifiers = Modifiers {
            shift,
            ctrl,
            alt: any_alt,
        };
        for key in pressed {
            if let Some(pos) = callback_pressed.iter().position(|&k| k == key) {
                callback_pressed.remove(pos);
                continue;
            }
            // Only keys still physically held can produce an auto-repeat.
            if !still_down(key) {
                continue;
            }
            if shortcut_key(key).is_some_and(|k| self.shortcut_keys.owns(k)) {
                if key == MinifbKey::Tab && ctrl && !any_alt {
                    actions.push(FrameAction::Tab(if shift {
                        TabShortcut::Previous
                    } else {
                        TabShortcut::Next
                    }));
                }
                continue;
            }
            if !ctrl && any_alt {
                match key {
                    MinifbKey::Down => {
                        actions.push(FrameAction::Chrome(ChromeShortcut::ScrollDown));
                        continue;
                    }
                    MinifbKey::Up => {
                        actions.push(FrameAction::Chrome(ChromeShortcut::ScrollUp));
                        continue;
                    }
                    MinifbKey::Left | MinifbKey::Right | MinifbKey::R => continue,
                    _ => {}
                }
            }
            if ctrl && !any_alt && matches!(key, MinifbKey::A | MinifbKey::C | MinifbKey::X | MinifbKey::L) {
                continue;
            }
            if key == MinifbKey::Escape {
                continue;
            }
            if let Some(engine_key) = translate_with_modifiers(key, modifiers) {
                actions.push(FrameAction::Key(KeyEvent::with_modifiers(
                    engine_key, modifiers,
                )));
            }
        }
        actions
    }

    #[cfg(test)]
    fn drain(&mut self, pressed: Vec<MinifbKey>, modifiers: Modifiers) -> Vec<KeyEvent> {
        let left_alt = modifiers.alt && !self.modifier_keys[5];
        let right_alt = modifiers.alt && self.modifier_keys[5];
        self.drain_actions(
            pressed,
            |_| true,
            left_alt,
            right_alt,
            modifiers.ctrl,
            modifiers.shift,
        )
        .into_iter()
        .filter_map(|action| match action {
            FrameAction::Key(event) => Some(event),
            _ => None,
        })
        .collect()
    }
}

impl InputCallback for CharacterQueue {
    fn add_char(&mut self, uni_char: u32) {
        // No source means no shortcut ownership, even while another key is
        // owned. Capture Shift NOW, without inheriting Ctrl/Alt.
        self.push_char(uni_char, true);
    }

    fn add_char_with_key(&mut self, uni_char: u32, source: Option<MinifbKey>, code: Option<u32>) {
        let modifiers = self.modifiers();
        // Text-first backends (such as Wayland's xkb_keysym_to_utf32) report
        // printable ASCII for Ctrl+L/A/C/V/X before `set_key_state_with_code`.
        // Those chords are emitted explicitly on key-down, so suppress the raw
        // text event here to prevent DOM leakage (Ctrl+L) or double-paste (Ctrl+V).
        // Windows AltGr (LeftCtrl+RightAlt) has `modifiers.alt == true` and is
        // intentionally unaffected.
        if modifiers.ctrl
            && !modifiers.alt
            && matches!(
                source,
                Some(
                    MinifbKey::L
                        | MinifbKey::A
                        | MinifbKey::C
                        | MinifbKey::V
                        | MinifbKey::X
                )
            )
        {
            return;
        }
        if let Some(code) = code {
            if !self.down_codes.contains(&code) {
                self.shortcut_keys.native_key_state(
                    code,
                    source.and_then(shortcut_key),
                    true,
                    modifiers.ctrl,
                    modifiers.alt,
                );
            }
            if self.shortcut_keys.owns_physical(code) {
                return;
            }
            if self.modifier_keys[5] && !self.modifier_keys[4] {
                if let Some((pending_code, idx)) = self.pending_right_alt_r {
                    if pending_code == code
                        && self.actions.get(idx)
                            == Some(&FrameAction::Chrome(ChromeShortcut::Reload))
                    {
                        self.actions.remove(idx);
                        self.pending_right_alt_r = None;
                    }
                }
                self.altgr_text_codes.insert(code);
            }
        }
        let unassociated_ime = source.is_none() && code.is_none();
        self.push_char(uni_char, unassociated_ime);
    }

    fn set_ime_preedit(&mut self, text: &str, cursor: usize) {
        self.actions.push(FrameAction::ImePreedit {
            text: text.to_string(),
            cursor,
        });
        self.ime_activity = true;
    }

    fn notify_ime_activity(&mut self) {
        self.ime_activity = true;
    }

    fn set_key_state(&mut self, key: MinifbKey, down: bool) {
        self.set_key_state_with_code(key, down, key as u32);
    }

    fn set_key_state_with_code(&mut self, key: MinifbKey, down: bool, code: u32) {
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
        let was_down = if down {
            !self.down_codes.insert(code)
        } else {
            self.altgr_text_codes.remove(&code);
            if self.pending_right_alt_r.is_some_and(|(c, _)| c == code) {
                self.pending_right_alt_r = None;
            }
            self.down_codes.remove(&code)
        };
        // Only an initial key-down (never an OS auto-repeat of a key pressed
        // before Ctrl went down) may acquire shortcut ownership; key-up always
        // releases it.
        if !down || !was_down {
            self.shortcut_keys.native_key_state(
                code,
                shortcut_key(key),
                down,
                modifiers.ctrl,
                modifiers.alt,
            );
        }
        if !down || was_down {
            return;
        }
        // Record every initial key-down seen this frame so `drain_actions`
        // never re-processes a key pressed (and possibly released) before a
        // later modifier went down.
        self.callback_pressed_keys.push(key);

        let left_alt = self.modifier_keys[4];
        let right_alt = self.modifier_keys[5];
        let any_alt = left_alt || right_alt;

        if modifiers.ctrl && !any_alt {
            if let Some(tab_shortcut) = tab_shortcut_for(key, modifiers.shift) {
                self.actions.push(FrameAction::Tab(tab_shortcut));
                return;
            }
            if key == MinifbKey::L {
                self.actions
                    .push(FrameAction::Chrome(ChromeShortcut::FocusAddress));
                return;
            }
        }

        if any_alt && !modifiers.ctrl {
            let chrome = match key {
                MinifbKey::Left => Some(ChromeShortcut::Back),
                MinifbKey::Right => Some(ChromeShortcut::Forward),
                MinifbKey::Down => Some(ChromeShortcut::ScrollDown),
                MinifbKey::Up => Some(ChromeShortcut::ScrollUp),
                MinifbKey::R if left_alt => Some(ChromeShortcut::Reload),
                MinifbKey::R if right_alt && !self.altgr_text_codes.contains(&code) => {
                    let idx = self.actions.len();
                    self.actions
                        .push(FrameAction::Chrome(ChromeShortcut::Reload));
                    self.pending_right_alt_r = Some((code, idx));
                    return;
                }
                _ => None,
            };
            if let Some(chrome) = chrome {
                self.actions.push(FrameAction::Chrome(chrome));
                return;
            }
        }

        if !self.shortcut_keys.owns_physical(code)
            && !shortcut_key(key).is_some_and(|k| self.shortcut_keys.owns(k))
        {
            if let Some(engine_key) = translate_with_modifiers(key, modifiers) {
                self.actions.push(FrameAction::Key(KeyEvent::with_modifiers(
                    engine_key, modifiers,
                )));
            }
        }
    }
}

/// Collects window input and hands the engine a chronological stream of frame actions.
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

    /// Every tab shortcut, chrome shortcut and key event produced since the
    /// last frame in exact chronological callback order, plus genuine held-key
    /// auto-repeats.
    pub(crate) fn drain_actions(&self, window: &Window) -> Vec<FrameAction> {
        let left_alt = window.is_key_down(MinifbKey::LeftAlt);
        let right_alt = window.is_key_down(MinifbKey::RightAlt);
        let ctrl =
            window.is_key_down(MinifbKey::LeftCtrl) || window.is_key_down(MinifbKey::RightCtrl);
        let shift =
            window.is_key_down(MinifbKey::LeftShift) || window.is_key_down(MinifbKey::RightShift);
        self.characters.borrow_mut().drain_actions(
            window.get_keys_pressed(KeyRepeat::Yes),
            |key| window.is_key_down(key),
            left_alt,
            right_alt,
            ctrl,
            shift,
        )
    }

    /// True when an OS IME composition message was processed this frame.
    pub fn take_ime_activity(&self) -> bool {
        std::mem::replace(&mut self.characters.borrow_mut().ime_activity, false)
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
    fn set_key_state_with_code(&mut self, key: MinifbKey, down: bool, code: u32) {
        self.0.borrow_mut().set_key_state_with_code(key, down, code);
    }
    fn add_char_with_key(&mut self, uni_char: u32, source: Option<MinifbKey>, code: Option<u32>) {
        self.0
            .borrow_mut()
            .add_char_with_key(uni_char, source, code);
    }
    fn set_ime_preedit(&mut self, text: &str, cursor: usize) {
        self.0.borrow_mut().set_ime_preedit(text, cursor);
    }
    fn notify_ime_activity(&mut self) {
        self.0.borrow_mut().notify_ime_activity();
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

fn tab_shortcut_for(key: MinifbKey, shift: bool) -> Option<TabShortcut> {
    Some(match key {
        MinifbKey::T => TabShortcut::New,
        MinifbKey::W => TabShortcut::CloseActive,
        MinifbKey::Tab => {
            if shift {
                TabShortcut::Previous
            } else {
                TabShortcut::Next
            }
        }
        MinifbKey::Key1 => TabShortcut::Number(1),
        MinifbKey::Key2 => TabShortcut::Number(2),
        MinifbKey::Key3 => TabShortcut::Number(3),
        MinifbKey::Key4 => TabShortcut::Number(4),
        MinifbKey::Key5 => TabShortcut::Number(5),
        MinifbKey::Key6 => TabShortcut::Number(6),
        MinifbKey::Key7 => TabShortcut::Number(7),
        MinifbKey::Key8 => TabShortcut::Number(8),
        MinifbKey::Key9 => TabShortcut::Number(9),
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

/// Map a physical key to the engine's key names, including Ctrl+A/C/V/X
/// editing chords (which arrive as control characters on the text channel).
fn translate_with_modifiers(key: MinifbKey, modifiers: Modifiers) -> Option<EngineKey> {
    if let Some(named) = translate(key) {
        return Some(named);
    }
    if modifiers.ctrl && !modifiers.alt {
        return match key {
            MinifbKey::A => Some(EngineKey::Character('a')),
            MinifbKey::C => Some(EngineKey::Character('c')),
            MinifbKey::V => Some(EngineKey::Character('v')),
            MinifbKey::X => Some(EngineKey::Character('x')),
            _ => None,
        };
    }
    None
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

    struct TestQueue {
        inner: CharacterQueue,
        text_first: bool,
    }

    impl std::ops::Deref for TestQueue {
        type Target = CharacterQueue;
        fn deref(&self) -> &Self::Target {
            &self.inner
        }
    }
    impl std::ops::DerefMut for TestQueue {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.inner
        }
    }

    fn queue(text_first: bool) -> TestQueue {
        TestQueue {
            inner: CharacterQueue::default(),
            text_first,
        }
    }

    // Replay either native callback order, with source from the same event.
    fn press(queue: &mut TestQueue, key: MinifbKey, character: char) {
        if queue.text_first {
            queue.add_char_with_key(character as u32, Some(key), Some(key as u32));
        }
        queue.set_key_state(key, true);
        if !queue.text_first {
            queue.add_char_with_key(character as u32, Some(key), Some(key as u32));
        }
    }

    fn drain(queue: &mut TestQueue, keys: Vec<MinifbKey>) -> Vec<KeyEvent> {
        {
            let modifiers = queue.modifiers();
            queue.inner.drain(keys, modifiers)
        }
    }

    fn text(events: &[KeyEvent]) -> String {
        events
            .iter()
            .filter_map(KeyEvent::typed_character)
            .collect()
    }

    fn type_aw1b(queue: &mut TestQueue) {
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

    #[test]
    fn identical_key_names_and_characters_do_not_alias_physical_ownership() {
        let mut q = CharacterQueue::default();
        q.set_key_state(MinifbKey::LeftCtrl, true);
        q.set_key_state_with_code(MinifbKey::Key1, true, 1000);
        q.add_char_with_key('1' as u32, Some(MinifbKey::Key1), Some(1000));
        q.set_key_state(MinifbKey::LeftCtrl, false);
        q.set_key_state_with_code(MinifbKey::Key1, true, 1001);
        q.add_char_with_key('1' as u32, Some(MinifbKey::Key1), Some(1001));
        q.set_key_state_with_code(MinifbKey::Key1, false, 1001);
        q.add_char_with_key('1' as u32, Some(MinifbKey::Key1), Some(1000));
        assert_eq!(
            q.drain(vec![], Modifiers::NONE)
                .iter()
                .filter_map(KeyEvent::typed_character)
                .collect::<String>(),
            "1"
        );
        assert!(q.shortcut_keys.owns_physical(1000));
        q.set_key_state_with_code(MinifbKey::Unknown, false, 1000);
        assert!(!q.shortcut_keys.owns_physical(1000));
    }

    #[test]
    fn multi_scalar_events_never_borrow_another_physical_keys_owner() {
        for text_first in [false, true] {
            let mut queue = queue(text_first);
            queue.set_key_state(MinifbKey::LeftCtrl, true);
            press(&mut queue, MinifbKey::Key1, '1');
            queue.set_key_state(MinifbKey::LeftCtrl, false);
            press(&mut queue, MinifbKey::Key1, '1');
            for c in "你好🙂".chars() {
                queue.add_char_with_key(c as u32, None, None);
            }
            for c in "é漢字".chars() {
                queue.add_char_with_key(c as u32, Some(MinifbKey::Q), Some(MinifbKey::Q as u32));
            }
            // Identical text from the owned key is entirely suppressed,
            // including all scalars; no first-character token/Unicode guess.
            for c in "你好🙂".chars() {
                queue.add_char_with_key(
                    c as u32,
                    Some(MinifbKey::Key1),
                    Some(MinifbKey::Key1 as u32),
                );
            }
            assert_eq!(text(&drain(&mut queue, vec![])), "你好🙂é漢字");
            queue.set_key_state(MinifbKey::Key1, false);
            press(&mut queue, MinifbKey::Key1, '1');
            assert_eq!(text(&drain(&mut queue, vec![])), "1");
        }
    }

    #[test]
    fn unassociated_ime_text_keeps_its_own_modifier_snapshot() {
        let mut queue = queue(true);
        queue.add_char('字' as u32);
        queue.set_key_state(MinifbKey::LeftShift, true);
        queue.add_char('漢' as u32);
        queue.set_key_state(MinifbKey::LeftCtrl, true);
        press(&mut queue, MinifbKey::Tab, '\t');
        queue.set_key_state(MinifbKey::LeftCtrl, false);
        queue.set_key_state(MinifbKey::LeftShift, false);
        let events = drain(&mut queue, vec![]);
        assert_eq!(text(&events), "字漢");
        assert_eq!(events[0].modifiers, Modifiers::NONE);
        assert_eq!(events[1].modifiers, Modifiers::shift());
    }

    #[test]
    fn unmapped_unicode_reaches_only_the_active_address_editor_then_dom() {
        let mut tabs = Tabs::new(browser(), 800);
        tabs.active_mut().toolbar.focus_address("old draft");
        let mut queue = queue(false);
        queue.set_key_state(MinifbKey::LeftCtrl, true);
        press(&mut queue, MinifbKey::T, '\u{14}');
        tabs.apply(TabCommand::New, 10, || Ok(browser()));
        queue.set_key_state(MinifbKey::LeftCtrl, false);
        // T is still owned; IME text has no key-state callback at all.
        for c in "你好🙂".chars() {
            queue.add_char(c as u32);
        }
        tabs.active_mut()
            .handle_keys(drain(&mut queue, vec![]), false);
        assert_eq!(tabs.active().toolbar.editor().unwrap().value(), "你好🙂");
        assert_eq!(
            tabs.get(0).unwrap().toolbar.editor().unwrap().value(),
            "old draft"
        );
        for tab in tabs.iter_mut() {
            assert!(dom_api::outer_html(&tab.browser.document().dom).contains(">empty</p>"));
        }
        tabs.active_mut().toolbar.cancel();
        queue.add_char('字' as u32);
        tabs.active_mut()
            .handle_keys(drain(&mut queue, vec![]), false);
        assert!(dom_api::outer_html(&tabs.active().browser.document().dom).contains(">字</p>"));
        assert!(
            dom_api::outer_html(&tabs.get(0).unwrap().browser.document().dom)
                .contains(">empty</p>")
        );
    }
}

#[cfg(test)]
mod reviewer_c_regressions {
    use super::*;
    use crate::tabs::Tabs;
    use browser_engine::{
        browser::Browser,
        net::{MemoryLoader, Url},
        script::dom_api,
    };

    fn ordinary_text(events: &[KeyEvent]) -> String {
        events
            .iter()
            .filter_map(KeyEvent::typed_character)
            .collect()
    }

    #[test]
    fn wayland_unmapped_unicode_before_no_text_ctrl_tab_is_preserved() {
        let mut q = CharacterQueue::default();
        // minifb wayland handle_key emits Unicode for an unrecognized keysym,
        // then returns without set_key_state. Tab emits NO add_char (<32).
        q.add_char('字' as u32);
        q.set_key_state(MinifbKey::LeftCtrl, true);
        q.set_key_state(MinifbKey::Tab, true);
        q.set_key_state(MinifbKey::Tab, false);
        q.set_key_state(MinifbKey::LeftCtrl, false);
        let events = q.drain(vec![], Modifiers::NONE);
        assert_eq!(
            ordinary_text(&events),
            "字",
            "queued ordinary Unicode must not become Tab's text: {events:?}"
        );
    }

    #[test]
    fn wayland_unmapped_unicode_keeps_callback_time_modifiers() {
        let mut q = CharacterQueue::default();
        q.add_char('字' as u32);
        q.set_key_state(MinifbKey::LeftCtrl, true);
        q.add_char_with_key(
            '1' as u32,
            Some(MinifbKey::Key1),
            Some(MinifbKey::Key1 as u32),
        );
        q.set_key_state(MinifbKey::Key1, true);
        q.set_key_state(MinifbKey::Key1, false);
        q.set_key_state(MinifbKey::LeftCtrl, false);
        let events = q.drain(vec![], Modifiers::NONE);
        assert_eq!(
            ordinary_text(&events),
            "字",
            "ordinary Unicode must not gain a later Ctrl modifier: {events:?}"
        );
    }

    #[test]
    fn x11_unmapped_unicode_after_owned_repeat_is_preserved() {
        let mut q = CharacterQueue::default();
        q.set_key_state(MinifbKey::LeftCtrl, true);
        q.set_key_state(MinifbKey::Key1, true);
        q.add_char_with_key(
            '1' as u32,
            Some(MinifbKey::Key1),
            Some(MinifbKey::Key1 as u32),
        );
        q.set_key_state(MinifbKey::LeftCtrl, false);
        // Held 1 repeats after Ctrl release. Next Unicode keysym has no
        // recognized key callback in X11 update_key_state, only add_char.
        q.set_key_state(MinifbKey::Key1, true);
        q.add_char_with_key(
            '1' as u32,
            Some(MinifbKey::Key1),
            Some(MinifbKey::Key1 as u32),
        );
        q.add_char('字' as u32);
        q.set_key_state(MinifbKey::Key1, false);
        let events = q.drain(vec![], Modifiers::NONE);
        assert_eq!(
            ordinary_text(&events),
            "字",
            "unmapped Unicode must not inherit held digit ownership: {events:?}"
        );
    }

    #[test]
    fn e2_named_key_before_wm_char_in_same_frame_keeps_chronological_order() {
        let mut q = CharacterQueue::default();
        // Win32 message queue delivers WM_KEYDOWN(Backspace) first, then WM_CHAR('M').
        q.set_key_state_with_code(MinifbKey::Backspace, true, 0x00E);
        q.add_char_with_key('M' as u32, Some(MinifbKey::M), Some(0x032));
        let events = q.drain(vec![MinifbKey::Backspace], Modifiers::NONE);
        assert_eq!(
            events,
            vec![
                KeyEvent::new(EngineKey::Backspace),
                KeyEvent::character('M'),
            ]
        );
    }

    #[test]
    fn e2_interleaved_chars_and_named_keys_preserve_exact_arrival_sequence() {
        let mut q = CharacterQueue::default();
        q.add_char_with_key('A' as u32, Some(MinifbKey::A), Some(0x01E));
        q.set_key_state_with_code(MinifbKey::Backspace, true, 0x00E);
        q.set_key_state_with_code(MinifbKey::Backspace, false, 0x00E);
        q.add_char_with_key('B' as u32, Some(MinifbKey::B), Some(0x030));
        q.set_key_state_with_code(MinifbKey::Enter, true, 0x01C);
        let events = q.drain(vec![MinifbKey::Enter], Modifiers::NONE);
        assert_eq!(
            events,
            vec![
                KeyEvent::character('A'),
                KeyEvent::new(EngineKey::Backspace),
                KeyEvent::character('B'),
                KeyEvent::new(EngineKey::Enter),
            ]
        );
    }

    #[test]
    fn e3_subframe_keydown_and_keyup_in_same_drain_is_not_lost_or_duplicated() {
        let mut q = CharacterQueue::default();
        q.set_key_state_with_code(MinifbKey::Backspace, true, 0x00E);
        q.set_key_state_with_code(MinifbKey::Backspace, false, 0x00E);
        // Even if get_keys_pressed is empty (or reports the one-shot press),
        // exactly one Backspace event must be emitted.
        assert_eq!(
            q.drain(vec![], Modifiers::NONE),
            vec![KeyEvent::new(EngineKey::Backspace)]
        );

        q.set_key_state_with_code(MinifbKey::Backspace, true, 0x00E);
        q.set_key_state_with_code(MinifbKey::Backspace, false, 0x00E);
        assert_eq!(
            q.drain(vec![MinifbKey::Backspace], Modifiers::NONE),
            vec![KeyEvent::new(EngineKey::Backspace)]
        );
    }

    #[test]
    fn p1_ime_preedit_and_commit_update_dom_without_leaking_uncommitted_text() {
        let mut loader = MemoryLoader::new();
        loader.insert(
            "demo:///ime",
            r#"<html><body><input id="q"><p id="events">0</p><script>
            let count = 0;
            document.getElementById("q").addEventListener("input", function() {
                count += 1;
                document.getElementById("events").textContent = String(count);
            });
            </script></body></html>"#,
        );
        let b = Browser::open(Box::new(loader), &Url::parse("demo:///ime").unwrap()).unwrap();
        let mut tabs = Tabs::new(b, 800);
        let mut strip = crate::tab_strip::TabStrip::new(800);
        let mut notice = None;
        let mut cb = crate::clipboard::MemoryClipboard::new();
        let mut ime_composing = false;
        let mut press_on_page = false;
        let no_tab = || -> Result<Browser, browser_engine::net::LoadError> {
            panic!("unexpected tab open")
        };
        let q_path = dom_api::get_element_by_id(&tabs.active().browser.document().dom, "q").unwrap();
        tabs.active_mut().browser.click_node(&q_path);

        let mut q = CharacterQueue::default();
        // Start Bopomofo composition: preedit "ㄓㄨㄥ" with cursor at 3.
        q.set_ime_preedit("ㄓㄨㄥ", 3);
        assert!(q.ime_activity);
        let actions = q.drain_actions(vec![], |_| false, false, false, false, false);
        crate::dispatch_frame_actions(
            actions,
            &mut tabs,
            &mut strip,
            &no_tab,
            &mut notice,
            &mut cb,
            &mut ime_composing,
            false,
            true,
            &mut press_on_page,
        );
        assert!(ime_composing);

        // Preedit is visible on the element for inline rendering, NOT in value,
        // and zero `input` events have fired.
        let el = dom_api::node_at(&tabs.active().browser.document().dom, &q_path)
            .unwrap()
            .as_element()
            .unwrap();
        assert_eq!(el.ime_preedit(), Some(("ㄓㄨㄥ", 3)));
        assert_eq!(el.control_value(), "");

        // Cancel composition via Escape: preedit clears and value remains "".
        q.set_ime_preedit("", 0);
        let actions = q.drain_actions(vec![], |_| false, false, false, false, false);
        crate::dispatch_frame_actions(
            actions,
            &mut tabs,
            &mut strip,
            &no_tab,
            &mut notice,
            &mut cb,
            &mut ime_composing,
            true,
            true,
            &mut press_on_page,
        );
        assert!(!ime_composing);
        let el = dom_api::node_at(&tabs.active().browser.document().dom, &q_path)
            .unwrap()
            .as_element()
            .unwrap();
        assert_eq!(el.ime_preedit(), None);
        assert_eq!(el.control_value(), "");

        // Compose again and commit "中文😀" via unassociated IME commit callbacks.
        q.set_ime_preedit("ㄓㄨㄥㄨㄣˊ", 6);
        for ch in "中文😀".chars() {
            q.add_char_with_key(ch as u32, None, None);
        }
        q.set_ime_preedit("", 0);
        let actions = q.drain_actions(vec![], |_| false, false, false, false, false);
        crate::dispatch_frame_actions(
            actions,
            &mut tabs,
            &mut strip,
            &no_tab,
            &mut notice,
            &mut cb,
            &mut ime_composing,
            false,
            true,
            &mut press_on_page,
        );
        assert!(!ime_composing);

        let el = dom_api::node_at(&tabs.active().browser.document().dom, &q_path)
            .unwrap()
            .as_element()
            .unwrap();
        assert_eq!(el.ime_preedit(), None);
        assert_eq!(el.control_value(), "中文😀");
    }

    #[test]
    fn p2_clipboard_copy_cut_paste_and_password_guard_work_in_address_and_dom() {
        use crate::clipboard::{Clipboard, MemoryClipboard};

        let mut loader = MemoryLoader::new();
        loader.insert(
            "demo:///clip",
            r#"<html><body>
                <input id="txt" value="Hello">
                <input id="pwd" type="password" value="s3cr3t">
                <textarea id="area"></textarea>
            </body></html>"#,
        );
        let b = Browser::open(Box::new(loader), &Url::parse("demo:///clip").unwrap()).unwrap();
        let mut tabs = Tabs::new(b, 800);
        let mut cb = MemoryClipboard::with_text("初始剪貼簿");

        // 1. Address bar: focus, Ctrl+A (already selected), Ctrl+C, Ctrl+V.
        let current = tabs.active().browser.url().to_string();
        tabs.active_mut().toolbar.focus_address(&current);
        let mut q = CharacterQueue::default();
        q.set_key_state_with_code(MinifbKey::LeftCtrl, true, 0x01D);
        q.set_key_state_with_code(MinifbKey::C, true, 0x02E);
        q.set_key_state_with_code(MinifbKey::C, false, 0x02E);
        let events = q.drain(vec![], q.modifiers());
        tabs.active_mut()
            .handle_keys_with_clipboard(events, false, &mut cb);
        assert_eq!(cb.get_text().as_deref(), Some("demo:///clip"));

        cb.set_text("https://example.test/中文");
        q.set_key_state_with_code(MinifbKey::V, true, 0x02F);
        q.set_key_state_with_code(MinifbKey::V, false, 0x02F);
        q.set_key_state_with_code(MinifbKey::LeftCtrl, false, 0x01D);
        let events = q.drain(vec![], q.modifiers());
        tabs.active_mut()
            .handle_keys_with_clipboard(events, false, &mut cb);
        assert_eq!(
            tabs.active().toolbar.editor().unwrap().value(),
            "https://example.test/中文"
        );
        tabs.active_mut().toolbar.cancel();

        // 2. Password input: Ctrl+A then Ctrl+C / Ctrl+X must NEVER copy or cut password.
        cb.set_text("safe-clipboard");
        let pwd = dom_api::get_element_by_id(&tabs.active().browser.document().dom, "pwd").unwrap();
        tabs.active_mut().browser.click_node(&pwd);
        q.set_key_state_with_code(MinifbKey::LeftCtrl, true, 0x01D);
        q.set_key_state_with_code(MinifbKey::A, true, 0x01E);
        q.set_key_state_with_code(MinifbKey::A, false, 0x01E);
        q.set_key_state_with_code(MinifbKey::C, true, 0x02E);
        q.set_key_state_with_code(MinifbKey::C, false, 0x02E);
        q.set_key_state_with_code(MinifbKey::X, true, 0x02D);
        q.set_key_state_with_code(MinifbKey::X, false, 0x02D);
        q.set_key_state_with_code(MinifbKey::LeftCtrl, false, 0x01D);
        let events = q.drain(vec![], q.modifiers());
        tabs.active_mut()
            .handle_keys_with_clipboard(events, false, &mut cb);
        assert_eq!(cb.get_text().as_deref(), Some("safe-clipboard"));
        let pwd_el = dom_api::node_at(&tabs.active().browser.document().dom, &pwd)
            .unwrap()
            .as_element()
            .unwrap();
        assert_eq!(pwd_el.control_value(), "s3cr3t");

        // 3. Text input -> Cut ("Hello") -> Paste into textarea ("Hello\n中文").
        let txt = dom_api::get_element_by_id(&tabs.active().browser.document().dom, "txt").unwrap();
        tabs.active_mut().browser.click_node(&txt);
        q.set_key_state_with_code(MinifbKey::LeftCtrl, true, 0x01D);
        q.set_key_state_with_code(MinifbKey::A, true, 0x01E);
        q.set_key_state_with_code(MinifbKey::A, false, 0x01E);
        q.set_key_state_with_code(MinifbKey::X, true, 0x02D);
        q.set_key_state_with_code(MinifbKey::X, false, 0x02D);
        q.set_key_state_with_code(MinifbKey::LeftCtrl, false, 0x01D);
        let events = q.drain(vec![], q.modifiers());
        tabs.active_mut()
            .handle_keys_with_clipboard(events, false, &mut cb);
        assert_eq!(cb.get_text().as_deref(), Some("Hello"));
        let txt_el = dom_api::node_at(&tabs.active().browser.document().dom, &txt)
            .unwrap()
            .as_element()
            .unwrap();
        assert_eq!(txt_el.control_value(), "");

        let area =
            dom_api::get_element_by_id(&tabs.active().browser.document().dom, "area").unwrap();
        tabs.active_mut().browser.click_node(&area);
        cb.set_text("Hello\n台灣😀");
        q.set_key_state_with_code(MinifbKey::LeftCtrl, true, 0x01D);
        q.set_key_state_with_code(MinifbKey::V, true, 0x02F);
        q.set_key_state_with_code(MinifbKey::V, false, 0x02F);
        q.set_key_state_with_code(MinifbKey::LeftCtrl, false, 0x01D);
        let events = q.drain(vec![], q.modifiers());
        tabs.active_mut()
            .handle_keys_with_clipboard(events, false, &mut cb);
        let area_el = dom_api::node_at(&tabs.active().browser.document().dom, &area)
            .unwrap()
            .as_element()
            .unwrap();
        assert_eq!(area_el.control_value(), "Hello\n台灣😀");
    }

    #[test]
    fn cx1_altgr_character_reaches_page_and_does_not_trigger_reload() {
        let mut loader = MemoryLoader::new();
        loader.insert("demo:///altgr", r#"<html><body><input id="q"></body></html>"#);
        let b = Browser::open(Box::new(loader), &Url::parse("demo:///altgr").unwrap()).unwrap();
        let mut tabs = Tabs::new(b, 800);
        let mut strip = crate::tab_strip::TabStrip::new(800);
        let mut notice = None;
        let mut ime_composing = false;
        let mut press_on_page = false;
        let no_tab = || -> Result<Browser, browser_engine::net::LoadError> {
            panic!("unexpected tab open")
        };
        let q_path = dom_api::get_element_by_id(&tabs.active().browser.document().dom, "q").unwrap();
        tabs.active_mut().browser.click_node(&q_path);

        let mut cb = crate::clipboard::MemoryClipboard::new();
        let mut queue = CharacterQueue::default();
        // Windows AltGr (LeftCtrl + RightAlt) typing '@' and an AltGr+R layout char 'ŕ'.
        queue.set_key_state_with_code(MinifbKey::LeftCtrl, true, 0x01D);
        queue.set_key_state_with_code(MinifbKey::RightAlt, true, 0x138);
        queue.set_key_state_with_code(MinifbKey::Key2, true, 0x003);
        queue.add_char_with_key('@' as u32, Some(MinifbKey::Key2), Some(0x003));
        queue.set_key_state_with_code(MinifbKey::Key2, false, 0x003);
        queue.set_key_state_with_code(MinifbKey::R, true, 0x013);
        queue.add_char_with_key('ŕ' as u32, Some(MinifbKey::R), Some(0x013));
        queue.set_key_state_with_code(MinifbKey::R, false, 0x013);
        let actions = queue.drain_actions(
            vec![MinifbKey::LeftCtrl, MinifbKey::RightAlt, MinifbKey::Key2, MinifbKey::R],
            |k| matches!(k, MinifbKey::LeftCtrl | MinifbKey::RightAlt),
            false,
            true,
            true,
            false,
        );
        let (keep_open, cmd) = crate::dispatch_frame_actions(
            actions,
            &mut tabs,
            &mut strip,
            &no_tab,
            &mut notice,
            &mut cb,
            &mut ime_composing,
            false,
            false,
            &mut press_on_page,
        );
        assert!(keep_open);
        assert_eq!(cmd, None, "AltGr must not trigger browser Alt navigation");
        let q_el = dom_api::node_at(&tabs.active().browser.document().dom, &q_path)
            .unwrap()
            .as_element()
            .unwrap();
        assert_eq!(q_el.control_value(), "@ŕ");

        // Pure RightAlt+R producing an AltGr character (key-first AND text-first)
        // must cancel tentative Reload and deliver the character.
        queue.set_key_state_with_code(MinifbKey::LeftCtrl, false, 0x01D);
        queue.set_key_state_with_code(MinifbKey::R, true, 0x013);
        queue.add_char_with_key('ř' as u32, Some(MinifbKey::R), Some(0x013));
        queue.set_key_state_with_code(MinifbKey::R, false, 0x013);
        queue.add_char_with_key('®' as u32, Some(MinifbKey::R), Some(0x013));
        queue.set_key_state_with_code(MinifbKey::R, true, 0x013);
        queue.set_key_state_with_code(MinifbKey::R, false, 0x013);
        let actions = queue.drain_actions(
            vec![MinifbKey::R],
            |k| k == MinifbKey::RightAlt,
            false,
            true,
            false,
            false,
        );
        let (keep_open, cmd) = crate::dispatch_frame_actions(
            actions,
            &mut tabs,
            &mut strip,
            &no_tab,
            &mut notice,
            &mut cb,
            &mut ime_composing,
            false,
            false,
            &mut press_on_page,
        );
        assert!(keep_open);
        assert_eq!(cmd, None, "RightAlt+R AltGr text must cancel Reload");
        let q_el = dom_api::node_at(&tabs.active().browser.document().dom, &q_path)
            .unwrap()
            .as_element()
            .unwrap();
        assert_eq!(q_el.control_value(), "@ŕř®");

        // LeftAlt + Left arrow MUST still block page events and emit Back.
        queue.set_key_state_with_code(MinifbKey::RightAlt, false, 0x138);
        queue.set_key_state_with_code(MinifbKey::LeftAlt, true, 0x038);
        queue.set_key_state_with_code(MinifbKey::Left, true, 0x14B);
        let actions = queue.drain_actions(
            vec![MinifbKey::LeftAlt, MinifbKey::Left],
            |k| matches!(k, MinifbKey::LeftAlt | MinifbKey::Left),
            true,
            false,
            false,
            false,
        );
        let (keep_open, cmd) = crate::dispatch_frame_actions(
            actions,
            &mut tabs,
            &mut strip,
            &no_tab,
            &mut notice,
            &mut cb,
            &mut ime_composing,
            false,
            false,
            &mut press_on_page,
        );
        assert!(keep_open);
        assert_eq!(cmd, Some(crate::toolbar::Command::Back));
    }

    #[test]
    fn cx2_ime_commit_while_ctrl_held_preserves_cjk_without_triggering_shortcuts() {
        let mut loader = MemoryLoader::new();
        loader.insert(
            "demo:///ime_ctrl",
            r#"<html><body><input id="q" value="keep"></body></html>"#,
        );
        let b = Browser::open(Box::new(loader), &Url::parse("demo:///ime_ctrl").unwrap()).unwrap();
        let mut tabs = Tabs::new(b, 800);
        let q_path = dom_api::get_element_by_id(&tabs.active().browser.document().dom, "q").unwrap();
        tabs.active_mut().browser.click_node(&q_path);

        let mut cb = crate::clipboard::MemoryClipboard::with_text("PASTE");
        let mut q = CharacterQueue::default();
        q.set_key_state_with_code(MinifbKey::LeftCtrl, true, 0x01D);
        // Unassociated IME GCS_RESULTSTR arrives while LeftCtrl is down.
        for ch in "中文a".chars() {
            q.add_char_with_key(ch as u32, None, None);
        }
        q.set_key_state_with_code(MinifbKey::LeftCtrl, false, 0x01D);
        let events = q.drain(vec![], Modifiers::NONE);
        tabs.active_mut()
            .handle_keys_with_clipboard(events, false, &mut cb);
        let q_el = dom_api::node_at(&tabs.active().browser.document().dom, &q_path)
            .unwrap()
            .as_element()
            .unwrap();
        assert_eq!(q_el.control_value(), "中文akeep");
    }

    #[test]
    fn cx3_tab_switch_and_toolbar_focus_clear_stale_ime_preedit() {
        let mut loader = MemoryLoader::new();
        loader.insert("demo:///a", r#"<html><body><input id="q"></body></html>"#);
        let mut loader_b = MemoryLoader::new();
        loader_b.insert("demo:///b", r#"<html><body><input id="r"></body></html>"#);
        let b1 = Browser::open(Box::new(loader), &Url::parse("demo:///a").unwrap()).unwrap();
        let b2 = Browser::open(Box::new(loader_b), &Url::parse("demo:///b").unwrap()).unwrap();
        let mut tabs = Tabs::new(b1, 800);
        let q_path = dom_api::get_element_by_id(&tabs.get(0).unwrap().browser.document().dom, "q").unwrap();
        tabs.active_mut().browser.click_node(&q_path);
        tabs.active_mut().set_ime_preedit("ㄓㄨㄥ", 3);

        // Switching to a second tab clears preedit on the outgoing tab.
        tabs.open(b2);
        let el0 = dom_api::node_at(&tabs.get(0).unwrap().browser.document().dom, &q_path)
            .unwrap()
            .as_element()
            .unwrap();
        assert_eq!(el0.ime_preedit(), None);

        // Focusing the toolbar address bar via Ctrl+L clears DOM preedit on the same tab.
        let r_path = dom_api::get_element_by_id(&tabs.active().browser.document().dom, "r").unwrap();
        tabs.active_mut().browser.click_node(&r_path);
        tabs.active_mut().set_ime_preedit("ㄆ", 1);
        tabs.active_mut().focus_address();
        let el1 = dom_api::node_at(&tabs.active().browser.document().dom, &r_path)
            .unwrap()
            .as_element()
            .unwrap();
        assert_eq!(el1.ime_preedit(), None);

        // Password fields never store IME preedit.
        let mut loader2 = MemoryLoader::new();
        loader2.insert("demo:///pwd", r#"<html><body><input id="p" type="password"></body></html>"#);
        let bp = Browser::open(Box::new(loader2), &Url::parse("demo:///pwd").unwrap()).unwrap();
        let mut pwd_tab = crate::tabs::Tab::new(bp, 800);
        let p_path = dom_api::get_element_by_id(&pwd_tab.browser.document().dom, "p").unwrap();
        pwd_tab.browser.click_node(&p_path);
        pwd_tab.set_ime_preedit("ㄓㄨㄥ", 3);
        let pw_el = dom_api::node_at(&pwd_tab.browser.document().dom, &p_path)
            .unwrap()
            .as_element()
            .unwrap();
        assert_eq!(pw_el.ime_preedit(), None);
    }

    #[test]
    fn cx4_ctrl_v_dispatches_input_before_keyup() {
        let mut loader = MemoryLoader::new();
        loader.insert(
            "demo:///events",
            r#"<html><body><input id="q"><p id="log"></p><script>
            var q = document.getElementById("q");
            var log = [];
            function record(kind) {
                log.push(kind + ":" + q.value);
                document.getElementById("log").textContent = log.join("|");
            }
            q.addEventListener("keydown", function() { record("keydown"); });
            q.addEventListener("input", function() { record("input"); });
            q.addEventListener("keyup", function() { record("keyup"); });
            </script></body></html>"#,
        );
        let b = Browser::open(Box::new(loader), &Url::parse("demo:///events").unwrap()).unwrap();
        let mut tabs = Tabs::new(b, 800);
        let q_path = dom_api::get_element_by_id(&tabs.active().browser.document().dom, "q").unwrap();
        tabs.active_mut().browser.click_node(&q_path);

        let mut cb = crate::clipboard::MemoryClipboard::with_text("hi");
        let mut q = CharacterQueue::default();
        q.set_key_state_with_code(MinifbKey::LeftCtrl, true, 0x01D);
        q.set_key_state_with_code(MinifbKey::V, true, 0x02F);
        q.set_key_state_with_code(MinifbKey::V, false, 0x02F);
        q.set_key_state_with_code(MinifbKey::LeftCtrl, false, 0x01D);
        let events = q.drain(vec![], Modifiers::NONE);
        tabs.active_mut()
            .handle_keys_with_clipboard(events, false, &mut cb);
        assert!(dom_api::outer_html(&tabs.active().browser.document().dom)
            .contains("keydown:|input:hi|keyup:hi"));
    }

    fn two_seeded_editing_tabs() -> (Tabs, crate::tab_strip::TabStrip) {
        let mut loader1 = MemoryLoader::new();
        loader1.insert("demo:///t0", "<html><title>T0</title></html>");
        let mut loader2 = MemoryLoader::new();
        loader2.insert("demo:///t1", "<html><title>T1</title></html>");
        let b0 = Browser::open(Box::new(loader1), &Url::parse("demo:///t0").unwrap()).unwrap();
        let b1 = Browser::open(Box::new(loader2), &Url::parse("demo:///t1").unwrap()).unwrap();

        let mut tabs = Tabs::new(b0, 800);
        tabs.active_mut().toolbar.focus_address("seed");
        tabs.active_mut()
            .handle_keys(vec![KeyEvent::new(EngineKey::End)], false);
        tabs.open(b1);
        tabs.active_mut().toolbar.focus_address("seed");
        tabs.active_mut()
            .handle_keys(vec![KeyEvent::new(EngineKey::End)], false);
        assert_eq!(tabs.active_index(), 1);
        (tabs, crate::tab_strip::TabStrip::new(800))
    }

    #[test]
    fn char_typed_before_ctrl_number_in_same_frame_stays_on_previous_tab() {
        let (mut tabs, mut strip) = two_seeded_editing_tabs();
        let mut cb = crate::clipboard::MemoryClipboard::new();
        let mut q = CharacterQueue::default();
        let mut notice = None;
        let mut ime_composing = false;
        let mut press_on_page = false;
        let no_tab = || -> Result<Browser, browser_engine::net::LoadError> {
            panic!("unexpected tab open")
        };

        // Rapid typing on Tab 1: 't', 'w', '1', immediately followed in the same
        // message pump by a complete sub-frame Ctrl+1 switching back to Tab 0
        // (ctrl is already false by end of frame) and 'x' typed onto Tab 0.
        for &(key, ch, code) in &[
            (MinifbKey::T, 't', 0x014),
            (MinifbKey::W, 'w', 0x011),
            (MinifbKey::Key1, '1', 0x002),
        ] {
            q.set_key_state_with_code(key, true, code);
            q.add_char_with_key(ch as u32, Some(key), Some(code));
            q.set_key_state_with_code(key, false, code);
        }
        q.set_key_state_with_code(MinifbKey::LeftCtrl, true, 0x01D);
        q.set_key_state_with_code(MinifbKey::Key1, true, 0x002);
        q.add_char_with_key('1' as u32, Some(MinifbKey::Key1), Some(0x002));
        q.set_key_state_with_code(MinifbKey::Key1, false, 0x002);
        q.set_key_state_with_code(MinifbKey::LeftCtrl, false, 0x01D);
        q.set_key_state_with_code(MinifbKey::X, true, 0x02D);
        q.add_char_with_key('x' as u32, Some(MinifbKey::X), Some(0x02D));
        q.set_key_state_with_code(MinifbKey::X, false, 0x02D);

        let actions = q.drain_actions(
            vec![
                MinifbKey::T,
                MinifbKey::W,
                MinifbKey::Key1,
                MinifbKey::LeftCtrl,
                MinifbKey::X,
            ],
            |_| false,
            false,
            false,
            false,
            false,
        );
        let (keep_open, cmd) = crate::dispatch_frame_actions(
            actions,
            &mut tabs,
            &mut strip,
            &no_tab,
            &mut notice,
            &mut cb,
            &mut ime_composing,
            false,
            false,
            &mut press_on_page,
        );
        assert!(keep_open);
        assert_eq!(cmd, None);
        assert_eq!(tabs.active_index(), 0);
        assert_eq!(tabs.get(1).unwrap().toolbar.editor().unwrap().value(), "seedtw1");
        assert_eq!(tabs.get(0).unwrap().toolbar.editor().unwrap().value(), "seedx");
    }

    #[test]
    fn m2_r4_split_frame_plain_1_then_ctrl_does_not_fabricate_ctrl_1_or_corrupt_tabs() {
        // Exact root cause of X11 CI failure ("rapid switching with focused address editing"):
        // Frame N receives plain '1' down+up (E3 keeps Key1 latched in keys_pressed),
        // then LeftCtrl down arrives before the frame ends, while Key1 of Ctrl+1 lands
        // in Frame N+1. Previously polling is_key_down(Ctrl) && is_key_pressed(Key1)
        // fabricated a premature Ctrl+1 on Frame N and dumped '1' into Tab 0 ("tw11"/"tw").
        let (mut tabs, mut strip) = two_seeded_editing_tabs();
        let mut cb = crate::clipboard::MemoryClipboard::new();
        let mut q = CharacterQueue::default();
        let mut notice = None;
        let mut ime_composing = false;
        let mut press_on_page = false;
        let no_tab = || -> Result<Browser, browser_engine::net::LoadError> {
            panic!("unexpected tab open")
        };

        // Frame N on Tab 1: 't', 'w', '1' (down+up), then LeftCtrl down.
        for &(key, ch, code) in &[
            (MinifbKey::T, 't', 28),
            (MinifbKey::W, 'w', 25),
            (MinifbKey::Key1, '1', 10),
        ] {
            q.set_key_state_with_code(key, true, code);
            q.add_char_with_key(ch as u32, Some(key), Some(code));
            q.set_key_state_with_code(key, false, code);
        }
        q.set_key_state_with_code(MinifbKey::LeftCtrl, true, 37);

        let frame_n = q.drain_actions(
            vec![MinifbKey::T, MinifbKey::W, MinifbKey::Key1, MinifbKey::LeftCtrl],
            |k| k == MinifbKey::LeftCtrl,
            false,
            false,
            true,
            false,
        );
        let (keep_open, cmd) = crate::dispatch_frame_actions(
            frame_n,
            &mut tabs,
            &mut strip,
            &no_tab,
            &mut notice,
            &mut cb,
            &mut ime_composing,
            false,
            false,
            &mut press_on_page,
        );
        assert!(keep_open);
        assert_eq!(cmd, None);
        assert_eq!(
            tabs.active_index(),
            1,
            "plain '1' released before LeftCtrl must stay on Tab 1"
        );
        assert_eq!(tabs.get(1).unwrap().toolbar.editor().unwrap().value(), "seedtw1");
        assert_eq!(tabs.get(0).unwrap().toolbar.editor().unwrap().value(), "seed");

        // Frame N+1: Key1 down while LeftCtrl is still held -> real Ctrl+1 switches to Tab 0,
        // then releases and types "tw1" into Tab 0.
        q.set_key_state_with_code(MinifbKey::Key1, true, 10);
        q.add_char_with_key('1' as u32, Some(MinifbKey::Key1), Some(10));
        q.set_key_state_with_code(MinifbKey::Key1, false, 10);
        q.set_key_state_with_code(MinifbKey::LeftCtrl, false, 37);
        for &(key, ch, code) in &[
            (MinifbKey::T, 't', 28),
            (MinifbKey::W, 'w', 25),
            (MinifbKey::Key1, '1', 10),
        ] {
            q.set_key_state_with_code(key, true, code);
            q.add_char_with_key(ch as u32, Some(key), Some(code));
            q.set_key_state_with_code(key, false, code);
        }
        let frame_n1 = q.drain_actions(
            vec![MinifbKey::Key1, MinifbKey::T, MinifbKey::W],
            |_| false,
            false,
            false,
            false,
            false,
        );
        let (keep_open, cmd) = crate::dispatch_frame_actions(
            frame_n1,
            &mut tabs,
            &mut strip,
            &no_tab,
            &mut notice,
            &mut cb,
            &mut ime_composing,
            false,
            false,
            &mut press_on_page,
        );
        assert!(keep_open);
        assert_eq!(cmd, None);
        assert_eq!(tabs.active_index(), 0);
        assert_eq!(tabs.get(0).unwrap().toolbar.editor().unwrap().value(), "seedtw1");
        assert_eq!(tabs.get(1).unwrap().toolbar.editor().unwrap().value(), "seedtw1");
    }

    #[test]
    fn m2_r4_twenty_rapid_tab_switches_under_all_frame_splits_stay_exact() {
        let (mut tabs, mut strip) = two_seeded_editing_tabs();
        let mut cb = crate::clipboard::MemoryClipboard::new();
        let mut q = CharacterQueue::default();
        let mut notice = None;
        let mut ime_composing = false;
        let mut press_on_page = false;
        let no_tab = || -> Result<Browser, browser_engine::net::LoadError> {
            panic!("unexpected tab open")
        };

        let mut expected = [String::from("seed"), String::from("seed")];

        let mut pump_frame = |q: &mut CharacterQueue,
                              tabs: &mut Tabs,
                              strip: &mut crate::tab_strip::TabStrip,
                              pressed: Vec<MinifbKey>,
                              ctrl_held: bool| {
            let actions = q.drain_actions(
                pressed,
                |k| ctrl_held && k == MinifbKey::LeftCtrl,
                false,
                false,
                ctrl_held,
                false,
            );
            let (keep_open, cmd) = crate::dispatch_frame_actions(
                actions,
                tabs,
                strip,
                &no_tab,
                &mut notice,
                &mut cb,
                &mut ime_composing,
                false,
                false,
                &mut press_on_page,
            );
            assert!(keep_open);
            assert_eq!(cmd, None);
        };

        for iter in 0..20usize {
            let target = iter % 2;
            let (num_key, num_ch, num_code) = if target == 0 {
                (MinifbKey::Key1, '1', 10)
            } else {
                (MinifbKey::Key2, '2', 11)
            };

            match iter % 3 {
                0 => {
                    // Split A: Ctrl down in prior frame's tail, digit in next frame.
                    q.set_key_state_with_code(MinifbKey::LeftCtrl, true, 37);
                    pump_frame(&mut q, &mut tabs, &mut strip, vec![MinifbKey::Key1, MinifbKey::LeftCtrl], true);

                    q.set_key_state_with_code(num_key, true, num_code);
                    q.add_char_with_key(num_ch as u32, Some(num_key), Some(num_code));
                    q.set_key_state_with_code(num_key, false, num_code);
                    q.set_key_state_with_code(MinifbKey::LeftCtrl, false, 37);
                    for &(k, c, code) in &[
                        (MinifbKey::T, 't', 28),
                        (MinifbKey::W, 'w', 25),
                        (MinifbKey::Key1, '1', 10),
                    ] {
                        q.set_key_state_with_code(k, true, code);
                        q.add_char_with_key(c as u32, Some(k), Some(code));
                        q.set_key_state_with_code(k, false, code);
                    }
                    pump_frame(
                        &mut q,
                        &mut tabs,
                        &mut strip,
                        vec![num_key, MinifbKey::T, MinifbKey::W, MinifbKey::Key1],
                        false,
                    );
                }
                1 => {
                    // Split B: complete sub-frame Ctrl+N tap + "tw" in Frame 1, "'1' + next Ctrl down" in Frame 2.
                    q.set_key_state_with_code(MinifbKey::LeftCtrl, true, 37);
                    q.set_key_state_with_code(num_key, true, num_code);
                    q.add_char_with_key(num_ch as u32, Some(num_key), Some(num_code));
                    q.set_key_state_with_code(num_key, false, num_code);
                    q.set_key_state_with_code(MinifbKey::LeftCtrl, false, 37);
                    for &(k, c, code) in &[(MinifbKey::T, 't', 28), (MinifbKey::W, 'w', 25)] {
                        q.set_key_state_with_code(k, true, code);
                        q.add_char_with_key(c as u32, Some(k), Some(code));
                        q.set_key_state_with_code(k, false, code);
                    }
                    pump_frame(
                        &mut q,
                        &mut tabs,
                        &mut strip,
                        vec![MinifbKey::LeftCtrl, num_key, MinifbKey::T, MinifbKey::W],
                        false,
                    );
                    q.set_key_state_with_code(MinifbKey::Key1, true, 10);
                    q.add_char_with_key('1' as u32, Some(MinifbKey::Key1), Some(10));
                    q.set_key_state_with_code(MinifbKey::Key1, false, 10);
                    pump_frame(&mut q, &mut tabs, &mut strip, vec![MinifbKey::Key1], false);
                }
                _ => {
                    // Split C: Ctrl+N and full "tw1" all inside one single frame.
                    q.set_key_state_with_code(MinifbKey::LeftCtrl, true, 37);
                    q.set_key_state_with_code(num_key, true, num_code);
                    q.add_char_with_key(num_ch as u32, Some(num_key), Some(num_code));
                    q.set_key_state_with_code(num_key, false, num_code);
                    q.set_key_state_with_code(MinifbKey::LeftCtrl, false, 37);
                    for &(k, c, code) in &[
                        (MinifbKey::T, 't', 28),
                        (MinifbKey::W, 'w', 25),
                        (MinifbKey::Key1, '1', 10),
                    ] {
                        q.set_key_state_with_code(k, true, code);
                        q.add_char_with_key(c as u32, Some(k), Some(code));
                        q.set_key_state_with_code(k, false, code);
                    }
                    pump_frame(
                        &mut q,
                        &mut tabs,
                        &mut strip,
                        vec![MinifbKey::LeftCtrl, num_key, MinifbKey::T, MinifbKey::W, MinifbKey::Key1],
                        false,
                    );
                }
            }
            expected[target].push_str("tw1");
        }

        assert_eq!(tabs.get(0).unwrap().toolbar.editor().unwrap().value(), expected[0]);
        assert_eq!(tabs.get(1).unwrap().toolbar.editor().unwrap().value(), expected[1]);
    }

    #[test]
    fn m2_r4_plain_keys_before_ctrl_or_alt_never_trigger_spurious_shortcuts() {
        let mut loader = MemoryLoader::new();
        loader.insert("demo:///home", "<html><body><input id=\"q\"></body></html>");
        let b = Browser::open(Box::new(loader), &Url::parse("demo:///home").unwrap()).unwrap();
        let mut tabs = Tabs::new(b, 800);
        let mut strip = crate::tab_strip::TabStrip::new(800);
        let q_path = dom_api::get_element_by_id(&tabs.active().browser.document().dom, "q").unwrap();
        tabs.active_mut().browser.click_node(&q_path);

        let mut cb = crate::clipboard::MemoryClipboard::new();
        let mut q = CharacterQueue::default();
        let mut notice = None;
        let mut ime_composing = false;
        let mut press_on_page = false;
        let no_tab = || -> Result<Browser, browser_engine::net::LoadError> {
            panic!("plain 't' released before Ctrl must never open a new tab")
        };

        // Plain 'w', 't', 'l' pressed and released BEFORE LeftCtrl goes down in the same frame:
        // must type "wtl" and must NOT close the window (Ctrl+W), open a tab (Ctrl+T), or focus address (Ctrl+L).
        for &(key, ch, code) in &[
            (MinifbKey::W, 'w', 0x011),
            (MinifbKey::T, 't', 0x014),
            (MinifbKey::L, 'l', 0x026),
        ] {
            q.set_key_state_with_code(key, true, code);
            q.add_char_with_key(ch as u32, Some(key), Some(code));
            q.set_key_state_with_code(key, false, code);
        }
        q.set_key_state_with_code(MinifbKey::LeftCtrl, true, 0x01D);

        let actions = q.drain_actions(
            vec![MinifbKey::W, MinifbKey::T, MinifbKey::L, MinifbKey::LeftCtrl],
            |k| k == MinifbKey::LeftCtrl,
            false,
            false,
            true,
            false,
        );
        let (keep_open, cmd) = crate::dispatch_frame_actions(
            actions,
            &mut tabs,
            &mut strip,
            &no_tab,
            &mut notice,
            &mut cb,
            &mut ime_composing,
            false,
            false,
            &mut press_on_page,
        );
        assert!(keep_open, "plain 'w' before Ctrl must never close the window");
        assert_eq!(cmd, None);
        assert_eq!(tabs.len(), 1);
        assert!(!tabs.active().toolbar.is_editing(), "plain 'l' before Ctrl must not focus address");
        let q_el = dom_api::node_at(&tabs.active().browser.document().dom, &q_path)
            .unwrap()
            .as_element()
            .unwrap();
        assert_eq!(q_el.control_value(), "wtl");

        // Release Ctrl, then plain 'r' down+up before LeftAlt goes down: must type 'r', NOT Reload.
        q.set_key_state_with_code(MinifbKey::LeftCtrl, false, 0x01D);
        q.set_key_state_with_code(MinifbKey::R, true, 0x013);
        q.add_char_with_key('r' as u32, Some(MinifbKey::R), Some(0x013));
        q.set_key_state_with_code(MinifbKey::R, false, 0x013);
        q.set_key_state_with_code(MinifbKey::LeftAlt, true, 0x038);
        let actions = q.drain_actions(
            vec![MinifbKey::R, MinifbKey::LeftAlt],
            |k| k == MinifbKey::LeftAlt,
            true,
            false,
            false,
            false,
        );
        let (keep_open, cmd) = crate::dispatch_frame_actions(
            actions,
            &mut tabs,
            &mut strip,
            &no_tab,
            &mut notice,
            &mut cb,
            &mut ime_composing,
            false,
            false,
            &mut press_on_page,
        );
        assert!(keep_open);
        assert_eq!(cmd, None, "plain 'r' before LeftAlt must never trigger Reload");
        let q_el = dom_api::node_at(&tabs.active().browser.document().dom, &q_path)
            .unwrap()
            .as_element()
            .unwrap();
        assert_eq!(q_el.control_value(), "wtlr");
    }

    #[test]
    fn m2_r4_escape_in_address_bar_blocks_trailing_page_leak_and_held_ctrl_tab_repeats_cleanly() {
        let mut loader1 = MemoryLoader::new();
        loader1.insert("demo:///a", r#"<html><body><input id="q"></body></html>"#);
        let mut loader2 = MemoryLoader::new();
        loader2.insert("demo:///b", r#"<html><body><input id="r"></body></html>"#);
        let b0 = Browser::open(Box::new(loader1), &Url::parse("demo:///a").unwrap()).unwrap();
        let b1 = Browser::open(Box::new(loader2), &Url::parse("demo:///b").unwrap()).unwrap();
        let mut tabs = Tabs::new(b0, 800);
        let q_path = dom_api::get_element_by_id(&tabs.active().browser.document().dom, "q").unwrap();
        tabs.active_mut().browser.click_node(&q_path);
        tabs.active_mut().focus_address();
        tabs.open(b1);
        tabs.select(0);
        let mut strip = crate::tab_strip::TabStrip::new(800);

        let mut cb = crate::clipboard::MemoryClipboard::new();
        let mut q = CharacterQueue::default();
        let mut notice = None;
        let mut ime_composing = false;
        let mut press_on_page = false;
        let no_tab = || -> Result<Browser, browser_engine::net::LoadError> {
            panic!("unexpected tab open")
        };

        // Escape cancels address bar and drops trailing 'x' in the same frame without closing window.
        q.set_key_state_with_code(MinifbKey::Escape, true, 0x001);
        q.set_key_state_with_code(MinifbKey::Escape, false, 0x001);
        q.set_key_state_with_code(MinifbKey::X, true, 0x02D);
        q.add_char_with_key('x' as u32, Some(MinifbKey::X), Some(0x02D));
        q.set_key_state_with_code(MinifbKey::X, false, 0x02D);
        let actions = q.drain_actions(
            vec![MinifbKey::Escape, MinifbKey::X],
            |_| false,
            false,
            false,
            false,
            false,
        );
        let (keep_open, cmd) = crate::dispatch_frame_actions(
            actions,
            &mut tabs,
            &mut strip,
            &no_tab,
            &mut notice,
            &mut cb,
            &mut ime_composing,
            false,
            false,
            &mut press_on_page,
        );
        assert!(keep_open);
        assert_eq!(cmd, None);
        assert!(!tabs.active().toolbar.is_editing());
        let q_el = dom_api::node_at(&tabs.get(0).unwrap().browser.document().dom, &q_path)
            .unwrap()
            .as_element()
            .unwrap();
        assert_eq!(q_el.control_value(), "");

        // Held Ctrl+Tab repeats switch tabs while Ctrl is held, and stop (no DOM leak)
        // when Ctrl is released before Tab.
        q.set_key_state_with_code(MinifbKey::LeftCtrl, true, 0x01D);
        q.set_key_state_with_code(MinifbKey::Tab, true, 0x00F);
        let first = q.drain_actions(
            vec![MinifbKey::LeftCtrl, MinifbKey::Tab],
            |k| matches!(k, MinifbKey::LeftCtrl | MinifbKey::Tab),
            false,
            false,
            true,
            false,
        );
        assert_eq!(first, vec![FrameAction::Tab(TabShortcut::Next)]);
        let (keep_open, _) = crate::dispatch_frame_actions(
            first,
            &mut tabs,
            &mut strip,
            &no_tab,
            &mut notice,
            &mut cb,
            &mut ime_composing,
            false,
            false,
            &mut press_on_page,
        );
        assert!(keep_open);
        assert_eq!(tabs.active_index(), 1);

        // Repeat frame while Ctrl+Tab are both still physically down.
        let repeat = q.drain_actions(
            vec![MinifbKey::Tab],
            |k| matches!(k, MinifbKey::LeftCtrl | MinifbKey::Tab),
            false,
            false,
            true,
            false,
        );
        assert_eq!(repeat, vec![FrameAction::Tab(TabShortcut::Next)]);

        // Ctrl released while Tab stays held: owned Tab repeat must emit NOTHING.
        q.set_key_state_with_code(MinifbKey::LeftCtrl, false, 0x01D);
        let staggered = q.drain_actions(
            vec![MinifbKey::Tab],
            |k| k == MinifbKey::Tab,
            false,
            false,
            false,
            false,
        );
        assert!(staggered.is_empty(), "staggered Tab after Ctrl release must never leak");
    }

    #[test]
    fn m2_r4_ctrl_w_closing_non_last_composing_tab_clears_ime_composing_for_escape() {
        // Reviewer C MAJOR-1: closing Tab 0 of 2 keeps active_index == 0 while changing
        // tabs.len(), so ime_composing must still reset to false.
        let mut loader1 = MemoryLoader::new();
        loader1.insert("demo:///a", r#"<html><body><input id="q"></body></html>"#);
        let mut loader2 = MemoryLoader::new();
        loader2.insert("demo:///b", r#"<html><body><input id="r"></body></html>"#);
        let b0 = Browser::open(Box::new(loader1), &Url::parse("demo:///a").unwrap()).unwrap();
        let b1 = Browser::open(Box::new(loader2), &Url::parse("demo:///b").unwrap()).unwrap();
        let mut tabs = Tabs::new(b0, 800);
        tabs.open(b1);
        tabs.select(0);
        let mut strip = crate::tab_strip::TabStrip::new(800);
        let mut cb = crate::clipboard::MemoryClipboard::new();
        let mut q = CharacterQueue::default();
        let mut notice = None;
        let mut ime_composing = true;
        let mut press_on_page = false;
        let no_tab = || -> Result<Browser, browser_engine::net::LoadError> {
            panic!("unexpected tab open")
        };

        q.set_key_state_with_code(MinifbKey::LeftCtrl, true, 0x01D);
        q.set_key_state_with_code(MinifbKey::W, true, 0x011);
        q.set_key_state_with_code(MinifbKey::W, false, 0x011);
        q.set_key_state_with_code(MinifbKey::LeftCtrl, false, 0x01D);
        let actions = q.drain_actions(vec![], |_| false, false, false, false, false);
        let (keep_open, _) = crate::dispatch_frame_actions(
            actions,
            &mut tabs,
            &mut strip,
            &no_tab,
            &mut notice,
            &mut cb,
            &mut ime_composing,
            true,
            false,
            &mut press_on_page,
        );
        assert!(keep_open);
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs.active_index(), 0);
        assert!(!ime_composing, "closing active Tab 0 must clear ime_composing");

        // Next frame: plain Escape on surviving non-composing tab must close window.
        q.set_key_state_with_code(MinifbKey::Escape, true, 0x001);
        q.set_key_state_with_code(MinifbKey::Escape, false, 0x001);
        let actions = q.drain_actions(vec![], |_| false, false, false, false, false);
        let had_ime = ime_composing;
        let (keep_open, _) = crate::dispatch_frame_actions(
            actions,
            &mut tabs,
            &mut strip,
            &no_tab,
            &mut notice,
            &mut cb,
            &mut ime_composing,
            had_ime,
            false,
            &mut press_on_page,
        );
        assert!(!keep_open, "Escape must close window after composing tab was closed");
    }

    #[test]
    fn m2_r4_chrome_back_flushes_chronologically_before_ctrl_l_navigate_and_scroll() {
        // Reviewer C MAJOR-2: Alt+Left followed by Ctrl+L + URL + Enter in one frame
        // must run Back FIRST and then Navigate to c.html, never rolling back afterward.
        let mut loader = MemoryLoader::new();
        loader.insert("demo:///a.html", "<title>A</title><div style=\"height:2000px\">a</div>");
        loader.insert("demo:///b.html", "<title>B</title><div style=\"height:2000px\">b</div>");
        loader.insert("demo:///c.html", "<title>C</title><div style=\"height:2000px\">c</div>");
        let mut b = Browser::open(Box::new(loader), &Url::parse("demo:///a.html").unwrap()).unwrap();
        b.navigate(&Url::parse("demo:///b.html").unwrap()).unwrap();
        let mut tabs = Tabs::new(b, 800);
        let mut strip = crate::tab_strip::TabStrip::new(800);
        let mut cb = crate::clipboard::MemoryClipboard::new();
        let mut q = CharacterQueue::default();
        let mut notice = None;
        let mut ime_composing = false;
        let mut press_on_page = false;
        let no_tab = || -> Result<Browser, browser_engine::net::LoadError> {
            panic!("unexpected tab open")
        };

        // Alt+Left (Back to a.html), then Ctrl+L, type demo:///c.html, press Enter.
        q.set_key_state_with_code(MinifbKey::LeftAlt, true, 0x038);
        q.set_key_state_with_code(MinifbKey::Left, true, 0x14B);
        q.set_key_state_with_code(MinifbKey::Left, false, 0x14B);
        q.set_key_state_with_code(MinifbKey::LeftAlt, false, 0x038);

        q.set_key_state_with_code(MinifbKey::LeftCtrl, true, 0x01D);
        q.set_key_state_with_code(MinifbKey::L, true, 0x026);
        q.set_key_state_with_code(MinifbKey::L, false, 0x026);
        q.set_key_state_with_code(MinifbKey::LeftCtrl, false, 0x01D);

        for ch in "demo:///c.html".chars() {
            q.add_char(ch as u32);
        }
        q.set_key_state_with_code(MinifbKey::Enter, true, 0x01C);
        q.set_key_state_with_code(MinifbKey::Enter, false, 0x01C);

        let actions = q.drain_actions(vec![], |_| false, false, false, false, false);
        let (keep_open, cmd) = crate::dispatch_frame_actions(
            actions,
            &mut tabs,
            &mut strip,
            &no_tab,
            &mut notice,
            &mut cb,
            &mut ime_composing,
            false,
            false,
            &mut press_on_page,
        );
        assert!(keep_open);
        if let Some(cmd) = cmd {
            tabs.active_mut().run_command(cmd);
        }
        assert_eq!(tabs.active().browser.url().to_string(), "demo:///c.html");

        // Also Alt+Left followed by Alt+Down in one frame runs Back (resetting scroll_y)
        // BEFORE scrolling down by 40px.
        tabs.active_mut().scroll_y = 200.0;
        q.set_key_state_with_code(MinifbKey::LeftAlt, true, 0x038);
        q.set_key_state_with_code(MinifbKey::Left, true, 0x14B);
        q.set_key_state_with_code(MinifbKey::Left, false, 0x14B);
        q.set_key_state_with_code(MinifbKey::Down, true, 0x150);
        q.set_key_state_with_code(MinifbKey::Down, false, 0x150);
        q.set_key_state_with_code(MinifbKey::LeftAlt, false, 0x038);
        let actions = q.drain_actions(vec![], |_| false, false, false, false, false);
        let (keep_open, cmd) = crate::dispatch_frame_actions(
            actions,
            &mut tabs,
            &mut strip,
            &no_tab,
            &mut notice,
            &mut cb,
            &mut ime_composing,
            false,
            false,
            &mut press_on_page,
        );
        assert!(keep_open);
        assert_eq!(cmd, None);
        assert_eq!(tabs.active().browser.url().to_string(), "demo:///a.html");
        assert_eq!(tabs.active().scroll_y, 40.0);
    }

    #[test]
    fn m2_r4_wayland_text_first_ctrl_l_and_ctrl_v_do_not_leak_or_double_paste() {
        // Reviewer C MAJOR-3: Wayland emits printable ASCII via add_char_with_key BEFORE
        // set_key_state_with_code for Ctrl+L and Ctrl+V. Ctrl+L must not leak keydown('l')
        // to the DOM, Ctrl+V must paste once, and Windows AltGr (LeftCtrl+RightAlt+L) must
        // still deliver layout characters like 'ł'.
        let mut loader = MemoryLoader::new();
        loader.insert(
            "demo:///wl",
            r#"<html><body><input id="q"><p id="log">empty</p><script>
            var log = "";
            document.body.addEventListener("keydown", function(e) {
                log += e.key;
                document.getElementById("log").textContent = log;
            });
            </script></body></html>"#,
        );
        let b = Browser::open(Box::new(loader), &Url::parse("demo:///wl").unwrap()).unwrap();
        let mut tabs = Tabs::new(b, 800);
        let mut strip = crate::tab_strip::TabStrip::new(800);
        let q_path = dom_api::get_element_by_id(&tabs.active().browser.document().dom, "q").unwrap();
        tabs.active_mut().browser.click_node(&q_path);

        let mut cb = crate::clipboard::MemoryClipboard::with_text("hi");
        let mut q = CharacterQueue::default();
        let mut notice = None;
        let mut ime_composing = false;
        let mut press_on_page = false;
        let no_tab = || -> Result<Browser, browser_engine::net::LoadError> {
            panic!("unexpected tab open")
        };

        // 1. Wayland text-first Ctrl+V: must paste "hi" ONCE, not "hihi".
        q.set_key_state_with_code(MinifbKey::LeftCtrl, true, 29);
        q.add_char_with_key('v' as u32, Some(MinifbKey::V), Some(47));
        q.set_key_state_with_code(MinifbKey::V, true, 47);
        q.set_key_state_with_code(MinifbKey::V, false, 47);
        q.set_key_state_with_code(MinifbKey::LeftCtrl, false, 29);
        let actions = q.drain_actions(vec![], |_| false, false, false, false, false);
        crate::dispatch_frame_actions(
            actions,
            &mut tabs,
            &mut strip,
            &no_tab,
            &mut notice,
            &mut cb,
            &mut ime_composing,
            false,
            false,
            &mut press_on_page,
        );
        let q_el = dom_api::node_at(&tabs.active().browser.document().dom, &q_path)
            .unwrap()
            .as_element()
            .unwrap();
        assert_eq!(q_el.control_value(), "hi");

        // 2. Windows AltGr (LeftCtrl+RightAlt+L) typing 'ł' must still insert 'ł'.
        q.set_key_state_with_code(MinifbKey::LeftCtrl, true, 29);
        q.set_key_state_with_code(MinifbKey::RightAlt, true, 100);
        q.add_char_with_key('ł' as u32, Some(MinifbKey::L), Some(38));
        q.set_key_state_with_code(MinifbKey::L, true, 38);
        q.set_key_state_with_code(MinifbKey::L, false, 38);
        q.set_key_state_with_code(MinifbKey::RightAlt, false, 100);
        q.set_key_state_with_code(MinifbKey::LeftCtrl, false, 29);
        let actions = q.drain_actions(vec![], |_| false, false, false, false, false);
        crate::dispatch_frame_actions(
            actions,
            &mut tabs,
            &mut strip,
            &no_tab,
            &mut notice,
            &mut cb,
            &mut ime_composing,
            false,
            false,
            &mut press_on_page,
        );
        let q_el = dom_api::node_at(&tabs.active().browser.document().dom, &q_path)
            .unwrap()
            .as_element()
            .unwrap();
        assert_eq!(q_el.control_value(), "hił");
        assert!(!tabs.active().toolbar.is_editing());

        // 3. Wayland text-first Ctrl+L: must focus address bar without leaking 'l' to DOM.
        let log_before = dom_api::outer_html(&tabs.active().browser.document().dom);
        q.set_key_state_with_code(MinifbKey::LeftCtrl, true, 29);
        q.add_char_with_key('l' as u32, Some(MinifbKey::L), Some(38));
        q.set_key_state_with_code(MinifbKey::L, true, 38);
        q.set_key_state_with_code(MinifbKey::L, false, 38);
        q.set_key_state_with_code(MinifbKey::LeftCtrl, false, 29);
        let actions = q.drain_actions(vec![], |_| false, false, false, false, false);
        crate::dispatch_frame_actions(
            actions,
            &mut tabs,
            &mut strip,
            &no_tab,
            &mut notice,
            &mut cb,
            &mut ime_composing,
            false,
            false,
            &mut press_on_page,
        );
        assert!(tabs.active().toolbar.is_editing());
        assert_eq!(
            dom_api::outer_html(&tabs.active().browser.document().dom),
            log_before,
            "Wayland Ctrl+L must not leak keydown('l') to DOM before FocusAddress"
        );
    }

    #[test]
    fn m2_r4_plain_held_tab_os_repeat_after_ctrl_and_same_frame_tab_switch_ime_preedit() {
        // Reviewer C MINOR-1: plain Tab held before LeftCtrl goes down must NOT acquire
        // ShortcutKeyFilter ownership on an OS auto-repeat and emit TabShortcut::Next.
        let mut q = CharacterQueue::default();
        q.set_key_state_with_code(MinifbKey::Tab, true, 0x00F);
        let _ = q.drain_actions(vec![MinifbKey::Tab], |k| k == MinifbKey::Tab, false, false, false, false);
        q.set_key_state_with_code(MinifbKey::LeftCtrl, true, 0x01D);
        // OS auto-repeat fires set_key_state_with_code(Tab, true) while Tab is already down.
        q.set_key_state_with_code(MinifbKey::Tab, true, 0x00F);
        let actions = q.drain_actions(
            vec![MinifbKey::Tab],
            |k| matches!(k, MinifbKey::Tab | MinifbKey::LeftCtrl),
            false,
            false,
            true,
            false,
        );
        assert!(
            !actions.iter().any(|a| matches!(a, FrameAction::Tab(_))),
            "plain held Tab OS repeat after Ctrl must not hijack into TabShortcut: {actions:?}"
        );

        // Reviewer C MINOR-3: Ctrl+2 followed by set_ime_preedit("ㄓ", 1) in the same
        // frame must apply the preedit to Tab 1 (the destination tab), not wipe it on Tab 0.
        let mut loader1 = MemoryLoader::new();
        loader1.insert("demo:///a", r#"<html><body><input id="q"></body></html>"#);
        let mut loader2 = MemoryLoader::new();
        loader2.insert("demo:///b", r#"<html><body><input id="r"></body></html>"#);
        let b0 = Browser::open(Box::new(loader1), &Url::parse("demo:///a").unwrap()).unwrap();
        let b1 = Browser::open(Box::new(loader2), &Url::parse("demo:///b").unwrap()).unwrap();
        let mut tabs = Tabs::new(b0, 800);
        tabs.open(b1);
        let r_path = dom_api::get_element_by_id(&tabs.get(1).unwrap().browser.document().dom, "r").unwrap();
        tabs.active_mut().browser.click_node(&r_path);
        tabs.select(0);

        let mut strip = crate::tab_strip::TabStrip::new(800);
        let mut cb = crate::clipboard::MemoryClipboard::new();
        let mut q2 = CharacterQueue::default();
        let mut notice = None;
        let mut ime_composing = false;
        let mut press_on_page = false;
        let no_tab = || -> Result<Browser, browser_engine::net::LoadError> {
            panic!("unexpected tab open")
        };

        q2.set_key_state_with_code(MinifbKey::LeftCtrl, true, 0x01D);
        q2.set_key_state_with_code(MinifbKey::Key2, true, 0x003);
        q2.set_key_state_with_code(MinifbKey::Key2, false, 0x003);
        q2.set_key_state_with_code(MinifbKey::LeftCtrl, false, 0x01D);
        q2.set_ime_preedit("ㄓ", 1);

        let actions = q2.drain_actions(vec![], |_| false, false, false, false, false);
        let (keep_open, _) = crate::dispatch_frame_actions(
            actions,
            &mut tabs,
            &mut strip,
            &no_tab,
            &mut notice,
            &mut cb,
            &mut ime_composing,
            false,
            true,
            &mut press_on_page,
        );
        assert!(keep_open);
        assert_eq!(tabs.active_index(), 1);
        assert!(ime_composing);
        let r_el = dom_api::node_at(&tabs.get(1).unwrap().browser.document().dom, &r_path)
            .unwrap()
            .as_element()
            .unwrap();
        assert_eq!(r_el.ime_preedit(), Some(("ㄓ", 1)));
    }
}
