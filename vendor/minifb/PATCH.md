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
  from ISO_Left_Tab.
- Windows: WM_CHAR/WM_SYSCHAR supply the scan code from their own lParam.
  No previous-character/key guess is used. Native message-pump behavior and
  IME sequences still require Windows verification; no native test is claimed.
- macOS: NSEvent.keyCode accompanies each character through the native callback
  signature. No native test is claimed.

Changed upstream files: src/lib.rs; src/key_handler.rs; src/os/posix/{x11,wayland}.rs;
src/os/{windows,macos}/mod.rs; src/native/macosx/{MacMiniFB.m,OSXWindow.h,OSXWindow.m}.
Other vendored files are byte-identical to the published package.

The application snapshots modifiers at delivery, filters only explicitly
sourced owned keys, and treats the original add_char as unassociated text.
It has no pending-text pairing or global text-owned latch. Tests replay both
backend callback orders and keep the Reviewer C Unicode events unmapped.
