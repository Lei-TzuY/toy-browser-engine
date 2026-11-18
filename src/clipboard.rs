// ============================================================
//  clipboard.rs  —  Unicode clipboard access for --window
// ============================================================
//
//  Clipboard access is behind a small trait so unit tests always run against
//  [`MemoryClipboard`] and never read or overwrite the user's OS clipboard.
//  In `--window` mode [`SystemClipboard`] talks to Win32 `CF_UNICODETEXT`
//  exclusively when the user presses Ctrl+C, Ctrl+X or Ctrl+V.

/// Minimal text clipboard interface used by the address bar and page controls.
pub(crate) trait Clipboard {
    /// Read plain Unicode text from the clipboard, if available.
    fn get_text(&mut self) -> Option<String>;

    /// Write plain Unicode text to the clipboard.
    fn set_text(&mut self, text: &str) -> bool;
}

/// Isolated in-memory clipboard for tests and non-Windows fallback.
#[derive(Default, Debug, Clone)]
pub(crate) struct MemoryClipboard {
    text: Option<String>,
}

impl MemoryClipboard {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub(crate) fn with_text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
        }
    }
}

impl Clipboard for MemoryClipboard {
    fn get_text(&mut self) -> Option<String> {
        self.text.clone()
    }

    fn set_text(&mut self, text: &str) -> bool {
        self.text = Some(text.to_string());
        true
    }
}

/// Encode a Rust string as a NUL-terminated UTF-16 buffer for `CF_UNICODETEXT`.
pub(crate) fn encode_cf_unicodetext(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Decode a `CF_UNICODETEXT` UTF-16 slice bounded by `max_units`, stopping at
/// the first NUL unit and dropping any unpaired surrogates safely.
pub(crate) fn decode_cf_unicodetext(units: &[u16]) -> String {
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    char::decode_utf16(units[..end].iter().copied())
        .flatten()
        .collect()
}

/// OS clipboard used only in interactive `--window` mode on explicit user
/// Copy/Cut/Paste commands.
pub(crate) struct SystemClipboard {
    #[cfg(target_os = "windows")]
    hwnd: *mut std::ffi::c_void,
    #[cfg(not(target_os = "windows"))]
    fallback: MemoryClipboard,
}

impl SystemClipboard {
    pub(crate) fn new(window: &minifb::Window) -> Self {
        #[cfg(target_os = "windows")]
        {
            Self {
                hwnd: window.get_window_handle(),
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = window;
            Self {
                fallback: MemoryClipboard::new(),
            }
        }
    }
}

#[cfg(target_os = "windows")]
mod win32 {
    use std::ffi::c_void;

    const CF_UNICODETEXT: u32 = 13;
    const GMEM_MOVEABLE: u32 = 0x0002;

    #[link(name = "user32")]
    extern "system" {
        fn OpenClipboard(hwnd_new_owner: *mut c_void) -> i32;
        fn CloseClipboard() -> i32;
        fn EmptyClipboard() -> i32;
        fn IsClipboardFormatAvailable(format: u32) -> i32;
        fn GetClipboardData(format: u32) -> *mut c_void;
        fn SetClipboardData(format: u32, hmem: *mut c_void) -> *mut c_void;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalAlloc(flags: u32, bytes: usize) -> *mut c_void;
        fn GlobalLock(hmem: *mut c_void) -> *mut c_void;
        fn GlobalUnlock(hmem: *mut c_void) -> i32;
        fn GlobalFree(hmem: *mut c_void) -> *mut c_void;
        fn GlobalSize(hmem: *mut c_void) -> usize;
    }

    struct ClipboardGuard;

    impl ClipboardGuard {
        unsafe fn open(hwnd: *mut c_void) -> Option<Self> {
            if OpenClipboard(hwnd) != 0 {
                Some(ClipboardGuard)
            } else if !hwnd.is_null() && OpenClipboard(std::ptr::null_mut()) != 0 {
                Some(ClipboardGuard)
            } else {
                None
            }
        }
    }

    impl Drop for ClipboardGuard {
        fn drop(&mut self) {
            unsafe {
                CloseClipboard();
            }
        }
    }

    pub(super) fn read_unicode_text(hwnd: *mut c_void) -> Option<String> {
        unsafe {
            if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 {
                return None;
            }
            let _guard = ClipboardGuard::open(hwnd)?;
            let handle = GetClipboardData(CF_UNICODETEXT);
            if handle.is_null() {
                return None;
            }
            let byte_len = GlobalSize(handle);
            if byte_len < 2 {
                return Some(String::new());
            }
            let ptr = GlobalLock(handle) as *const u16;
            if ptr.is_null() {
                return None;
            }
            let units = std::slice::from_raw_parts(ptr, byte_len / 2);
            let text = super::decode_cf_unicodetext(units);
            GlobalUnlock(handle);
            Some(text)
        }
    }

    pub(super) fn write_unicode_text(hwnd: *mut c_void, text: &str) -> bool {
        unsafe {
            let _guard = match ClipboardGuard::open(hwnd) {
                Some(g) => g,
                None => return false,
            };
            if EmptyClipboard() == 0 {
                return false;
            }
            let units = super::encode_cf_unicodetext(text);
            let bytes = units.len() * std::mem::size_of::<u16>();
            let hmem = GlobalAlloc(GMEM_MOVEABLE, bytes);
            if hmem.is_null() {
                return false;
            }
            let dst = GlobalLock(hmem) as *mut u16;
            if dst.is_null() {
                GlobalFree(hmem);
                return false;
            }
            std::ptr::copy_nonoverlapping(units.as_ptr(), dst, units.len());
            GlobalUnlock(hmem);
            if SetClipboardData(CF_UNICODETEXT, hmem).is_null() {
                // Only free if SetClipboardData failed; otherwise the system
                // owns `hmem`.
                GlobalFree(hmem);
                return false;
            }
            true
        }
    }
}

impl Clipboard for SystemClipboard {
    fn get_text(&mut self) -> Option<String> {
        #[cfg(target_os = "windows")]
        {
            win32::read_unicode_text(self.hwnd)
        }
        #[cfg(not(target_os = "windows"))]
        {
            self.fallback.get_text()
        }
    }

    fn set_text(&mut self, text: &str) -> bool {
        #[cfg(target_os = "windows")]
        {
            win32::write_unicode_text(self.hwnd, text)
        }
        #[cfg(not(target_os = "windows"))]
        {
            self.fallback.set_text(text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_clipboard_roundtrips_unicode_and_surrogate_pairs() {
        let mut cb = MemoryClipboard::new();
        assert_eq!(cb.get_text(), None);
        assert!(cb.set_text("中文注音 ㄅㄆㄇ 😀🚀"));
        assert_eq!(cb.get_text().as_deref(), Some("中文注音 ㄅㄆㄇ 😀🚀"));
    }

    #[test]
    fn cf_unicodetext_encoding_stops_at_nul_and_preserves_non_bmp() {
        let original = "台灣瀏覽器 🌐 Test";
        let mut units = encode_cf_unicodetext(original);
        assert_eq!(*units.last().unwrap(), 0);
        // Trailing garbage after the NUL terminator must be ignored.
        units.extend_from_slice(&[0x0041, 0x0042]);
        assert_eq!(decode_cf_unicodetext(&units), original);
    }

    #[test]
    fn cf_unicodetext_drops_unpaired_surrogates_cleanly() {
        let units = [0x0041u16, 0xD83D, 0x0042, 0x0000];
        assert_eq!(decode_cf_unicodetext(&units), "AB");
    }
}
