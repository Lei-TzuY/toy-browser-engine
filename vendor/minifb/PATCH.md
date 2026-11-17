# Native character provenance patch

Upstream: minifb 0.28.0, crates.io package SHA-256
`d1a093126f2ed9012fc0b146934c97eb0273e54983680a8bf5309b6b4a365b32`.
The packaged source, manifests and licenses are retained. No registry cache
mutation or build-time source rewriting is needed. Cargo uses this path patch.

## Extension

`InputCallback::add_char_with_key(u32, Option<Key>, Option<u32>)` carries the producing native
key and stable native physical code independently of the character's Unicode value. Its default implementation
forwards to `add_char`, preserving behavior for existing minifb users. An unknown keysym still carries its own physical code; an IME commit without a
physical source has no code and never borrows another key's identity.
`set_key_state_with_code` supplies that same code on press/repeat/release.
KeyHandler remembers its original mapping until release, even if modifiers
change the keysym, and handles two codes mapped to the same Key independently.

- X11: `process_key` returns the mapped key from the same XKeyEvent, then
  Xutf8LookupString sends every codepoint with that source (including None).
  This preserves multi-codepoint XIM output without assuming every glyph gets
  a set_key_state callback.
- Wayland: resolve the same wl_keyboard event's key before emitting text,
  retaining the upstream character-before-key-state callback order. Preserve
  physical identity with the native wl_keyboard key code, so Shift changes
  to keysyms cannot strand ownership after key release. Shift+Tab is mapped
  from ISO_Left_Tab. `KeyHandler::update()` is invoked at the start of
  `Wayland::update()` before dispatching new display events, matching X11,
  Windows, and macOS lifecycle ordering.
- Windows: WM_CHAR/WM_SYSCHAR supply the scan code from their own lParam.
  No previous-character/key guess is used.
  - E1 (`VK_PROCESSKEY`): `WM_KEYDOWN` and `WM_SYSKEYDOWN` ignore keys whose
    low 16-bit virtual key code is `VK_PROCESSKEY` (`0xE5`), preventing IME
    composition keys (such as `Enter` candidate selection or `Backspace`
    preedit deletion) from leaking raw physical scan codes into key state.
  - E3 (Sub-frame key transitions): `KeyHandler` latches `keys_pressed` and
    `keys_released` across a single frame drain until the next `update()`, so
    `KeyDown` + `KeyUp` pairs arriving within the same `PeekMessageW` loop are
    reported once by `get_keys_pressed` and `get_keys_released`.
  - E4 (Resize & pointer coordinates): `WM_MOUSEMOVE` and `WM_{L,M,R}BUTTON*`
    update client-area mouse coordinates and button state directly from
    message `lParam`, `GetCursorPos` polling only updates coordinates when the
    OS screen cursor actually moves, and non-minimized `WM_SIZE` updates
    window `width` and `height`.
- macOS: NSEvent.keyCode accompanies each character through the native callback
  signature. No native test is claimed.

Changed upstream files: src/lib.rs; src/key_handler.rs; src/os/posix/{x11,wayland}.rs;
src/os/{windows,macos}/mod.rs; src/native/macosx/{MacMiniFB.m,OSXWindow.h,OSXWindow.m}.
Other vendored files are byte-identical to the published package.

The application snapshots modifiers at delivery, filters only explicitly
sourced owned keys, and treats the original add_char as unassociated text.
It has no pending-text pairing or global text-owned latch. Tests replay both
backend callback orders and keep the Reviewer C Unicode events unmapped.
