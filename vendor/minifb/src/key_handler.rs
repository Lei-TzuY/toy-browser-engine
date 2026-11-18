#[cfg(target_arch = "wasm32")]
extern crate instant;

#[cfg(target_arch = "wasm32")]
use instant::{Duration, Instant};
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};

use crate::{InputCallback, Key, KeyRepeat};

pub struct KeyHandler {
    pub key_callback: Option<Box<dyn InputCallback>>,
    prev_time: Instant,
    delta_time: Duration,
    keys: [bool; 512],
    keys_prev: [bool; 512],
    keys_pressed: [bool; 512],
    keys_released: [bool; 512],
    keys_down_duration: [f32; 512],
    key_repeat_delay: f32,
    key_repeat_rate: f32,
    native_keys: std::collections::BTreeMap<u32, Key>,
}

impl KeyHandler {
    pub fn new() -> KeyHandler {
        KeyHandler {
            key_callback: None,
            keys: [false; 512],
            keys_prev: [false; 512],
            keys_pressed: [false; 512],
            keys_released: [false; 512],
            keys_down_duration: [-1.0; 512],
            prev_time: Instant::now(),
            delta_time: Duration::from_secs(0),
            key_repeat_delay: 0.250,
            key_repeat_rate: 0.050,
            native_keys: std::collections::BTreeMap::new(),
        }
    }

    #[cfg(any(target_os = "redox", target_arch = "wasm32"))]
    #[inline]
    pub fn set_key_state(&mut self, key: Key, state: bool) {
        let idx = key as usize;
        let was_down = self.keys[idx];
        self.keys[idx] = state;
        if state && !was_down {
            self.keys_pressed[idx] = true;
            if self.keys_down_duration[idx] < 0.0 {
                self.keys_down_duration[idx] = 0.0;
            }
        } else if !state && (was_down || self.keys_pressed[idx]) {
            self.keys_released[idx] = true;
        }
        if let Some(cb) = &mut self.key_callback {
            cb.set_key_state(key, state);
        }
    }

    /// Preserve physical identity through repeats and layout/modifier changes.
    /// A release clears the key originally pressed, including unknown keysyms.
    pub fn set_key_state_with_code(&mut self, key: Key, state: bool, code: u32) {
        let key = if state {
            *self.native_keys.entry(code).or_insert(key)
        } else {
            self.native_keys.remove(&code).unwrap_or(key)
        };
        let idx = key as usize;
        let was_down = self.keys[idx];
        let now_down = state || self.native_keys.values().any(|k| *k == key);
        self.keys[idx] = now_down;
        if state && !was_down {
            self.keys_pressed[idx] = true;
            if self.keys_down_duration[idx] < 0.0 {
                self.keys_down_duration[idx] = 0.0;
            }
        } else if !now_down && (was_down || self.keys_pressed[idx]) {
            self.keys_released[idx] = true;
        }
        if let Some(cb) = &mut self.key_callback {
            cb.set_key_state_with_code(key, state, code);
        }
    }

    pub fn get_keys(&self) -> Vec<Key> {
        let mut keys: Vec<Key> = Vec::new();

        for (idx, is_down) in self.keys.iter().enumerate() {
            if *is_down {
                unsafe {
                    keys.push(std::mem::transmute(idx as u8));
                }
            }
        }

        keys
    }

    pub fn update(&mut self) {
        self.delta_time = self.prev_time.elapsed();
        self.prev_time = Instant::now();
        let delta_time = self.delta_time.as_secs_f32();

        for idx in 0..self.keys.len() {
            self.keys_pressed[idx] = false;
            self.keys_released[idx] = false;
            if self.keys[idx] {
                if self.keys_down_duration[idx] < 0.0 {
                    self.keys_down_duration[idx] = 0.0;
                } else {
                    self.keys_down_duration[idx] += delta_time.max(f32::MIN_POSITIVE);
                }
            } else {
                self.keys_down_duration[idx] = -1.0;
            }
            self.keys_prev[idx] = self.keys[idx];
        }
    }

    #[inline]
    pub fn set_input_callback(&mut self, callback: Box<dyn InputCallback>) {
        self.key_callback = Some(callback);
    }

    pub fn get_keys_pressed(&self, repeat: KeyRepeat) -> Vec<Key> {
        let mut keys: Vec<Key> = Vec::new();

        for (idx, is_down) in self.keys.iter().enumerate() {
            if (self.keys_pressed[idx] || *is_down) && self.is_key_index_pressed(idx, repeat) {
                unsafe {
                    keys.push(std::mem::transmute(idx as u8));
                }
            }
        }

        keys
    }

    pub fn get_keys_released(&self) -> Vec<Key> {
        let mut keys: Vec<Key> = Vec::new();

        for (idx, is_down) in self.keys.iter().enumerate() {
            if (!(*is_down) || self.keys_released[idx]) && self.is_key_index_released(idx) {
                unsafe {
                    keys.push(std::mem::transmute(idx as u8));
                }
            }
        }

        keys
    }

    #[inline]
    pub fn is_key_down(&self, key: Key) -> bool {
        self.keys[key as usize]
    }

    #[inline]
    pub fn set_key_repeat_delay(&mut self, delay: f32) {
        self.key_repeat_delay = delay;
    }

    #[inline]
    pub fn set_key_repeat_rate(&mut self, rate: f32) {
        self.key_repeat_rate = rate;
    }

    fn is_key_index_pressed(&self, index: usize, repeat: KeyRepeat) -> bool {
        if self.keys_pressed[index] {
            return true;
        }
        if !self.keys[index] {
            return false;
        }
        let t = self.keys_down_duration[index];

        if t == 0.0 {
            return true;
        }

        if repeat == KeyRepeat::Yes && t > self.key_repeat_delay {
            let delta_time = self.delta_time.as_secs_f32();
            let delay = self.key_repeat_delay;
            let rate = self.key_repeat_rate;
            if (((t - delay) % rate) > rate * 0.5)
                != (((t - delay - delta_time) % rate) > rate * 0.5)
            {
                return true;
            }
        }

        false
    }

    #[inline]
    pub fn is_key_pressed(&self, key: Key, repeat: KeyRepeat) -> bool {
        self.is_key_index_pressed(key as usize, repeat)
    }

    #[inline]
    pub fn is_key_released(&self, key: Key) -> bool {
        self.is_key_index_released(key as usize)
    }

    #[inline]
    fn is_key_index_released(&self, idx: usize) -> bool {
        self.keys_released[idx] || (self.keys_prev[idx] && !self.keys[idx])
    }

    /// Release all currently tracked physical/logical keys (for example on WM_KILLFOCUS)
    /// so modifiers and held keys cannot remain stuck after focus leaves the window.
    pub fn clear(&mut self) {
        let codes: Vec<(u32, Key)> = self.native_keys.iter().map(|(&c, &k)| (c, k)).collect();
        self.native_keys.clear();

        if let Some(cb) = &mut self.key_callback {
            let mut released_by_idx = [false; 512];
            for (code, key) in codes {
                released_by_idx[key as usize] = true;
                cb.set_key_state_with_code(key, false, code);
            }
            for (idx, was_down) in self.keys.iter().enumerate() {
                if *was_down && !released_by_idx[idx] && idx <= Key::Unknown as usize {
                    // Safety: indices `0..=Key::Unknown as usize` match valid `Key` discriminants.
                    let key: Key = unsafe { std::mem::transmute(idx as u8) };
                    cb.set_key_state(key, false);
                }
            }
        }

        for (idx, was_down) in self.keys.iter_mut().enumerate() {
            if *was_down || self.keys_pressed[idx] {
                self.keys_released[idx] = true;
            }
            *was_down = false;
            self.keys_prev[idx] = false;
            self.keys_pressed[idx] = false;
            self.keys_down_duration[idx] = -1.0;
        }
    }
}


#[cfg(test)]
mod provenance_tests {
    use super::*;

    #[test]
    fn unknown_release_clears_the_key_originally_pressed() {
        let mut h = KeyHandler::new();
        h.set_key_state_with_code(Key::Key1, true, 10);
        h.set_key_state_with_code(Key::Unknown, false, 10);
        assert!(!h.is_key_down(Key::Key1));
    }

    #[test]
    fn two_physical_keys_with_the_same_name_release_independently() {
        let mut h = KeyHandler::new();
        h.set_key_state_with_code(Key::Key1, true, 10);
        h.set_key_state_with_code(Key::Key1, true, 24);
        h.set_key_state_with_code(Key::Key1, false, 24);
        assert!(h.is_key_down(Key::Key1));
        h.set_key_state_with_code(Key::Key1, false, 10);
        assert!(!h.is_key_down(Key::Key1));
    }

    #[test]
    fn e3_same_frame_keydown_and_keyup_preserves_single_press_and_release() {
        let mut h = KeyHandler::new();
        h.update();
        // Both KeyDown and KeyUp arrive in the same frame's message loop
        // after update(), before the application polls get_keys_pressed.
        h.set_key_state_with_code(Key::Backspace, true, 0x00E);
        h.set_key_state_with_code(Key::Backspace, false, 0x00E);

        assert!(!h.is_key_down(Key::Backspace));
        assert!(h.is_key_pressed(Key::Backspace, KeyRepeat::No));
        assert_eq!(h.get_keys_pressed(KeyRepeat::Yes), vec![Key::Backspace]);
        assert!(h.is_key_released(Key::Backspace));
        assert_eq!(h.get_keys_released(), vec![Key::Backspace]);

        // Next frame's update() clears the one-shot press/release latches.
        h.update();
        assert!(!h.is_key_pressed(Key::Backspace, KeyRepeat::No));
        assert!(h.get_keys_pressed(KeyRepeat::Yes).is_empty());
        assert!(!h.is_key_released(Key::Backspace));
    }

    #[test]
    fn e3_held_key_fires_immediately_on_press_frame_and_not_again_before_repeat_delay() {
        let mut h = KeyHandler::new();
        h.update();
        h.set_key_state_with_code(Key::Enter, true, 0x01C);

        assert!(h.is_key_pressed(Key::Enter, KeyRepeat::No));
        assert_eq!(h.get_keys_pressed(KeyRepeat::Yes), vec![Key::Enter]);

        // Next frame while still held (< 250ms repeat delay): must not re-fire.
        h.update();
        assert!(h.is_key_down(Key::Enter));
        assert!(!h.is_key_pressed(Key::Enter, KeyRepeat::No));
        assert!(h.get_keys_pressed(KeyRepeat::Yes).is_empty());
    }

    #[test]
    fn clear_releases_held_keys_and_notifies_callback() {
        use std::cell::RefCell;
        use std::rc::Rc;

        #[derive(Default)]
        struct Log(Rc<RefCell<Vec<(Key, bool, u32)>>>);
        impl InputCallback for Log {
            fn add_char(&mut self, _: u32) {}
            fn set_key_state_with_code(&mut self, key: Key, state: bool, code: u32) {
                self.0.borrow_mut().push((key, state, code));
            }
        }

        let log = Rc::new(RefCell::new(Vec::new()));
        let mut h = KeyHandler::new();
        h.set_input_callback(Box::new(Log(Rc::clone(&log))));
        h.set_key_state_with_code(Key::LeftAlt, true, 0x038);
        h.set_key_state_with_code(Key::LeftCtrl, true, 0x01D);
        h.update();

        h.clear();
        assert!(!h.is_key_down(Key::LeftAlt));
        assert!(!h.is_key_down(Key::LeftCtrl));
        assert!(h.get_keys().is_empty());
        assert!(h.is_key_released(Key::LeftAlt));
        let events = log.borrow();
        assert!(events.contains(&(Key::LeftCtrl, false, 0x01D)));
        assert!(events.contains(&(Key::LeftAlt, false, 0x038)));
    }
}
