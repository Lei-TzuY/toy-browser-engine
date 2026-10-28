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
    shortcut_keys: ShortcutKeyFilter,
    modifier_keys: [bool; 6],
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
        if let Some(character) = char::from_u32(uni_char) {
            if !character.is_control() {
                let mut modifiers = self.modifiers();
                // AltGr produces text, including on systems reporting Ctrl+
                // right Alt. The text channel has already applied the layout.
                if self.modifier_keys[5] {
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
        // No source means no shortcut ownership, even while another key is
        // owned. Capture modifiers NOW, never when a later key/drain arrives.
        self.push_char(uni_char);
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
        self.push_char(uni_char);
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
    fn set_key_state_with_code(&mut self, key: MinifbKey, down: bool, code: u32) {
        self.0.borrow_mut().set_key_state_with_code(key, down, code);
    }
    fn add_char_with_key(&mut self, uni_char: u32, source: Option<MinifbKey>, code: Option<u32>) {
        self.0
            .borrow_mut()
            .add_char_with_key(uni_char, source, code);
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
}
