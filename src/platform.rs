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

/// Callback-time text and ownership. Native backends supply each character's
/// source explicitly; add_char alone means unassociated/IME text. There is no
/// adjacency latch, pending character pairing, or frame-duration assumption.
#[derive(Default)]
struct CharacterQueue {
    characters: Vec<KeyEvent>,
    callback_pressed_keys: Vec<MinifbKey>,
    down_codes: std::collections::BTreeSet<u32>,
    shortcut_keys: ShortcutKeyFilter,
    modifier_keys: [bool; 6],
    ime_preedit: Option<(String, usize)>,
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
                self.characters.push(KeyEvent::with_modifiers(
                    EngineKey::Character(character),
                    modifiers,
                ));
            }
        }
    }

    fn drain(&mut self, pressed: Vec<MinifbKey>, modifiers: Modifiers) -> Vec<KeyEvent> {
        let mut events = std::mem::take(&mut self.characters);
        let mut callback_pressed = std::mem::take(&mut self.callback_pressed_keys);
        for key in pressed {
            if shortcut_key(key).is_some_and(|key| self.shortcut_keys.owns(key)) {
                continue;
            }
            if let Some(pos) = callback_pressed.iter().position(|&k| k == key) {
                callback_pressed.remove(pos);
                continue;
            }
            if let Some(engine_key) = translate_with_modifiers(key, modifiers) {
                events.push(KeyEvent::with_modifiers(engine_key, modifiers));
            }
        }
        events
    }
}

impl InputCallback for CharacterQueue {
    fn add_char(&mut self, uni_char: u32) {
        // No source means no shortcut ownership, even while another key is
        // owned. Capture Shift NOW, without inheriting Ctrl/Alt.
        self.push_char(uni_char, true);
    }

    fn add_char_with_key(&mut self, uni_char: u32, source: Option<MinifbKey>, code: Option<u32>) {
        if let Some(code) = code {
            let modifiers = self.modifiers();
            self.shortcut_keys.native_key_state(
                code,
                source.and_then(shortcut_key),
                true,
                modifiers.ctrl,
                modifiers.alt,
            );
            if self.shortcut_keys.owns_physical(code) {
                return;
            }
        }
        let unassociated_ime = source.is_none() && code.is_none();
        self.push_char(uni_char, unassociated_ime);
    }

    fn set_ime_preedit(&mut self, text: &str, cursor: usize) {
        self.ime_preedit = Some((text.to_string(), cursor));
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
        self.shortcut_keys.native_key_state(
            code,
            shortcut_key(key),
            down,
            modifiers.ctrl,
            modifiers.alt,
        );
        let was_down = if down {
            !self.down_codes.insert(code)
        } else {
            self.down_codes.remove(&code)
        };
        if down
            && !was_down
            && !self.shortcut_keys.owns_physical(code)
            && !shortcut_key(key).is_some_and(|k| self.shortcut_keys.owns(k))
        {
            if let Some(engine_key) = translate_with_modifiers(key, modifiers) {
                self.characters
                    .push(KeyEvent::with_modifiers(engine_key, modifiers));
                self.callback_pressed_keys.push(key);
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
    /// characters and key-down transitions in callback order, then any
    /// remaining keys pressed/repeated this frame.
    pub fn drain(&self, window: &Window) -> Vec<KeyEvent> {
        self.characters.borrow_mut().drain(
            window.get_keys_pressed(KeyRepeat::Yes),
            self.modifiers(window),
        )
    }

    /// Latest IME composition update `(text, cursor_in_chars)` since the last
    /// check, if any. Empty `text` means the composition ended or was cleared.
    pub fn take_ime_preedit(&self) -> Option<(String, usize)> {
        self.characters.borrow_mut().ime_preedit.take()
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
        let q_path = dom_api::get_element_by_id(&tabs.active().browser.document().dom, "q").unwrap();
        tabs.active_mut().browser.click_node(&q_path);

        let mut q = CharacterQueue::default();
        // Start Bopomofo composition: preedit "ㄓㄨㄥ" with cursor at 3.
        q.set_ime_preedit("ㄓㄨㄥ", 3);
        assert!(q.ime_activity);
        let (preedit, cursor) = q.ime_preedit.take().unwrap();
        tabs.active_mut().set_ime_preedit(&preedit, cursor);

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
        let (preedit, cursor) = q.ime_preedit.take().unwrap();
        tabs.active_mut().set_ime_preedit(&preedit, cursor);
        let el = dom_api::node_at(&tabs.active().browser.document().dom, &q_path)
            .unwrap()
            .as_element()
            .unwrap();
        assert_eq!(el.ime_preedit(), None);
        assert_eq!(el.control_value(), "");

        // Compose again and commit "中文😀" via unassociated IME commit callbacks.
        q.set_ime_preedit("ㄓㄨㄥㄨㄣˊ", 6);
        let (preedit, cursor) = q.ime_preedit.take().unwrap();
        tabs.active_mut().set_ime_preedit(&preedit, cursor);
        for ch in "中文😀".chars() {
            q.add_char_with_key(ch as u32, None, None);
        }
        q.set_ime_preedit("", 0);
        let (preedit, cursor) = q.ime_preedit.take().unwrap();
        tabs.active_mut().set_ime_preedit(&preedit, cursor);
        let events = q.drain(vec![], Modifiers::NONE);
        tabs.active_mut().handle_keys(events, false);

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
        let q_path = dom_api::get_element_by_id(&tabs.active().browser.document().dom, "q").unwrap();
        tabs.active_mut().browser.click_node(&q_path);

        let mut cb = crate::clipboard::MemoryClipboard::new();
        let mut queue = CharacterQueue::default();
        // Windows AltGr (LeftCtrl + RightAlt) typing '@' and an AltGr+R layout char 'ŕ'.
        queue.set_key_state_with_code(MinifbKey::LeftCtrl, true, 0x01D);
        queue.set_key_state_with_code(MinifbKey::RightAlt, true, 0x138);
        queue.add_char_with_key('@' as u32, Some(MinifbKey::Key2), Some(0x003));
        queue.add_char_with_key('ŕ' as u32, Some(MinifbKey::R), Some(0x013));
        let events = queue.drain(vec![], queue.modifiers());
        let (cmd, allow_nav) = tabs
            .active_mut()
            .route_frame_keys(events, false, true, true, &mut cb);
        assert_eq!(cmd, None);
        assert!(!allow_nav, "AltGr must not trigger browser Alt navigation");
        let q_el = dom_api::node_at(&tabs.active().browser.document().dom, &q_path)
            .unwrap()
            .as_element()
            .unwrap();
        assert_eq!(q_el.control_value(), "@ŕ");

        // LeftAlt + Left arrow MUST still block page events and allow Alt nav.
        queue.set_key_state_with_code(MinifbKey::RightAlt, false, 0x138);
        queue.set_key_state_with_code(MinifbKey::LeftCtrl, false, 0x01D);
        queue.set_key_state_with_code(MinifbKey::LeftAlt, true, 0x038);
        queue.set_key_state_with_code(MinifbKey::Left, true, 0x14B);
        let events = queue.drain(vec![MinifbKey::Left], queue.modifiers());
        let (_, allow_nav) = tabs
            .active_mut()
            .route_frame_keys(events, true, false, false, &mut cb);
        assert!(allow_nav);
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
}
