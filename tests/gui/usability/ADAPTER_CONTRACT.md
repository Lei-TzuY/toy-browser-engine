# M2-R2 shared fixture and adapter contract

The R2 baseline starts at merged main `c379fd7e5bc2f8740cd9763cd60ee3706c1cb9b1`, after its required CI passed. Baseline receipts remain separate from later harness supplements and corrected-product runs.

`usability_corpus.py` is the one source of HTML, image bytes, routes, 26 case IDs, authored expectations and server request logging. Importing it has no side effects. `materialize(output)` writes a `site` directory and `corpus-manifest.json`. `FixtureServer(output).start()` creates a private loopback server on an OS-assigned port; `.base`, `.site`, `.manifest`, `.requests()` and `.close()` are public. Every observed request is also recorded in `requests.jsonl`, with method, path, decoded body, content type, sequence and time.

Use a distinct output directory and fresh owned browser/session for each run. Keep baseline and corrected receipts. Do not change fixture bytes between engines or before/after phases. Each adapter records engine/platform, exact source and executable hashes, capability limitations, profile/session isolation and raw input events. Neither a source substring nor injected JavaScript may substitute for an actual user action on the toy engine. Reference CDP may read live DOM and deliver real Input mouse/key/text events; distinguish this capability from the native toy GUI.

Viewport is 800×600. Toy client window is 800×668; tab strip is 32 and toolbar is 36, so page origin is client(0,68). Fixture body margin/padding is zero. A first `.control` occupies page(0,0) to(240,52), with 12px bottom margin. Click page(60,26). Forms place submit at page(60,90), using an explicit hidden display:none case marker. Checkbox click is page(12,12). Toolbar centers in client coordinates: Back(20,50), Forward(52,50), Reload(84,50). Scroll fixture is 700px spacer +100px link +50px tail; after scrolling to bottom, link click is page(60,480). CDP may use the actual selector's boundingClientRect center and record those observed coordinates.

The shared suite will call an adapter created with `(url, case_output_dir)` and use these methods:

| Method/property | Required behavior |
|---|---|
| `capabilities` | Dict including engine, platform, real_window, native_ime_composition, screenshot_includes_chrome, and navigation_method |
| `navigate(url)` | Toy: actual address-field interaction. Reference: record the exact CDP/native route used. |
| `click(x,y)` | Toy: translate page point to owned client. Reference: actual CDP Input mouse events at the same viewport coordinates. |
| `back()`, `forward()`, `reload()` | Toy: real window chrome keys/buttons; reference may use navigation APIs with capability recorded. |
| `type_text(value)` | Real key/text input events into the currently focused control; committed Unicode is distinct from IME. |
| `key(name)` | Names `Enter`, `Backspace`, `ArrowDown`, `Space`, `Escape`. Hold sampled toy inputs about 120ms. |
| `new_tab()`, `select_tab(index)`, `close_tab()` | Actual owned toy GUI tab operations, or reference CDP target operations with capability recorded; index is zero-based. |
| `scroll(notches)` | Negative means wheel down; no DOM scrolling injection for the toy. |
| `title()` | Actual current OS window caption (toy), or actual live document.title (reference). |
| `url()` | Current committed URL where observable; returnNone if unavailable and retain full actual caption. |
| `screenshot(path)` | Save only the owned browser client/page as PNG, with exact region/page origin in capabilities. |
| `tab_count()` | Actual observable tab count; returnNone if unsupported. Never infer count from per-page history `[1/1]`. |
| `close()` | Close only owned processes, windows, profiles and input state. |

Adapters may expose `.last_navigation_error` for an expected failed navigation. A case must record a failed action and continue recovery only when failure is the scenario's purpose. All other unexpected exceptions produce FAIL with raw details and screenshot if available. A missing capability produces BLOCKED/NOT_TESTED rather than PASS. U11 starts PARTIAL pending actual glyph-image review. U24 requires actual OS composition and stays BLOCKED unless that capability is present; Unicode committed text is a separate observation.

For U23, instantiate `UnavailableEndpoint` and keep its socket alive during the unavailable navigation. It owns a bound-but-not-listening loopback port, so the test cannot contact another local service. Close it after the case.

The case result schema is `id`, `name`, `expected`, `status`, `capabilities`, `fixture_sha256`, `actions` (time/input/outcome), `observations` (actual captions/URLs/requests/screenshots/pixels), and `limitations`. Use PASS/PARTIAL/FAIL/BLOCKED/NOT_TESTED. Count actual executed cases separately from blocked cases. Keep full raw results, including initial failures, and include actual reference observations independently from authored expectations.
