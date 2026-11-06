# M2-R2: trusted local browser usability

**Overall gate: HOLD. Native Windows interaction remains unverified.** The draft PR is reviewable source and evidence; these results do not authorize merging it or certify arbitrary-web browsing.

The immutable baseline is merged main `c379fd7e5bc2f8740cd9763cd60ee3706c1cb9b1`. The corrected production source is `f330aea360a30d22327ffa44b9c60c1c5f9e9024`, with `src` tree `e8607b53cb2c2497e73ccae2b0ee552dfc473851`. Later evidence/helper/CI changes preserve that Rust tree. The M2-R1 frozen corpus and v0.1.0 release assets were not changed.

## Observed coverage

| Mode | Actual executed | Result | Interpretation |
|---|---:|---|---|
| Independent Linux X11 baseline | 25 | 23 PASS, 1 PARTIAL, 1 FAIL; U24 BLOCKED | Actual window input, captions, pixels and server requests |
| Independent Linux X11 corrected source | 25 | 24 PASS, 1 PARTIAL; U24 BLOCKED | All 23 baseline passes retained; U18 now passes |
| Isolated Chrome reference | 25 | 24 PASS, U11 PARTIAL; U24 BLOCKED | Actual CDP input/live reads; glyph pixels separately reviewed |
| CLI rendering corroboration | 3 | U10/U12 PASS, U11 PARTIAL; other 23 NOT_TESTED | Initial rendering cannot establish interaction behavior |
| Native Windows no-input windows | 4 renders | 4 rendered, normal exits, foreground unchanged | Before/after Unicode and select painting only |
| Native Windows interaction | 0 valid verified cases | HOLD | First host had external input; alternate host ownership guards refused activation |

U11 remains PARTIAL in automated results. Default Linux has no installed CJK font. Independent configured-font captures, using the same DejaVu primary and installed `msyh.ttc` fallback before and after, show four tofu boxes becoming visible `中文測試`. The Latin, Greek and emoji page rows are byte-identical between those captures. Native default-font captures show Chinese and a monochrome grinning glyph after the fix. Actual Chrome captures show all authored text and colored emoji. These observations establish the pictured glyph coverage, not complete shaping or color-emoji support. U24 requires actual operating-system IME composition and stays BLOCKED.

The 20 fixture files are byte-identical across the valid Linux baseline/candidate and Chrome runs. The PNG is pinned to its original 105 bytes, SHA-256 `b7873540b4cceac16392704258f38824aba116620a6c791d93a61825b4d68a09`, avoiding different zlib/zlib-ng compression output on another host. HTML is written as UTF-8 bytes on every platform.

## Two scoped root causes

1. **Missing glyphs used the primary font's tofu.** Measurement and rasterization now select the same covering font per character. Existing primary-font glyphs keep their metrics. A fixed, lazy fallback list tries installed OS fonts, with optional `BROWSER_ENGINE_FALLBACK_FONT`; ASCII pages do not parse CJK fallback fonts. No system fonts are redistributed. Two tiny authored geometric fixtures and their reproducible generator verify coverage, metrics and lazy loading; the independent audit regenerated them byte-for-byte.
2. **Collapsed single-selects painted all option text and had no keyboard default.** The widget now paints its selected label, frame and arrow, sizes from its labels, and handles Up/Down/Home/End while skipping disabled options/groups. Its live JavaScript value and submitted value use the same canonical selection. User changes dispatch input/change; cancelled keys have no default. The original select's identity is preserved across keydown and input handlers, so removal cannot change a neighbouring select. Popup menus and multiple-select/listbox interaction remain incomplete.

U18's exact before observation was live `Select a`, concatenated `AlphaBetaGamma`, and server `pick=a` after ArrowDown/Enter. Afterward the screenshot shows a framed `Beta`, live `Select b`, and the server receives `pick=b`. Independent review also found a c9dd event-removal regression; its exact contiguous-select fixture failed before the f330 correction and passes afterward. Both receipts are retained.

| Actual capture | Before | After |
|---|---|---|
| Linux select, same keyboard actions | [raw PNG](images/select-linux-before.png) | [raw PNG](images/select-linux-after.png) |
| Linux Unicode, same configured fonts | [raw PNG](images/unicode-linux-before.png) | [raw PNG](images/unicode-linux-after.png) |
| Native Unicode, no input | [raw PNG](images/unicode-native-before.png) | [raw PNG](images/unicode-native-after.png) |

## Validation and retained failures

Linux `cargo test --locked`: **1906 passed, 0 failed, 0 ignored, 195 groups**. Native Windows: **1901 passed, 0 failed, 0 ignored, 195 groups**; five platform-specific Linux tests account for the difference. Independent C ran five additional event probes. Clippy exits 0 on clean merged main and the candidate. `cargo fmt --check` fails on both, listing the same 190 files. This comparison checks file lists only and does not prove the changed hunks are formatted; full formatting is not claimed to pass.

Existing X11 regression checks passed **52/52 tab assertions** and **11 shortcut-routing scenarios, 62 aggregate comparison units, 0 failures** with the documented bounded two-trial invocation. Several routing scenarios contain multiple assertions; 62 is the harness's aggregate, not 62 independent pages.

All relevant failed attempts remain distinguishable in the archive: A's initial tab-count recognizer ambiguity; early font captures before the first frame; an existing-smoke wrapper retaining WAYLAND_DISPLAY; missing private Xvfb `/tmp/p83/xkbcomp` alias before cases; initial select `.value` integration failures; the c9dd keydown-removal failure; native input interference and zero-executed ownership-guard refusals. Invalid C candidate provenance is marked and excluded from the final matrix. Unsafe desktop-blit pilot images, browser profiles, system fonts and compiled binaries are excluded.

The f330 push CI run [42067545560](https://github.com/Lei-TzuY/toy-browser-engine/actions/runs/42067545560) is **CANCELLED**, not green. Its check annotation says the X11 job exceeded `15m0s`; dependency installation alone took 11m11s. The test steps and artifact upload individually completed, but the job deadline expired. The final workflow gives that job 25 minutes and adds this actual X11 usability matrix. The final-head CI result is reported on the draft PR.

## Reproduction and raw evidence

Follow [the portable runner instructions](../../tests/gui/usability/README.md). Use a fresh output directory, release executable and trusted generated corpus. Every result retains authored expectations, exact observed values, actions, capabilities, executable/source hashes and screenshots. [matrix.json](matrix.json) contains all per-case expected/actual checks and status values. [raw-receipts-manifest.json](raw-receipts-manifest.json) hashes every archived file; [raw-receipts.tar.gz](raw-receipts.tar.gz) contains A's receipts, B's strict safe allowlist, and C's explicitly selected independent receipts.

The committed [independent summary](independent-summary-f330.json), [event probe source](reviewer_c_select_event_probe_v2.rs), [probe reproduction script](reproduce-reviewer-c-probes.sh) and [font-fixture audit](font-fixtures-audit.json) support independent replay. No temporary implementer fixture is needed to recreate the shared pages.

## Scenario matrix

The table uses C's valid baseline/final runs and B's actual Chrome reference. Exact captions, request values and pixel observations are in the linked JSON and raw receipts.

| ID | Scenario | Linux before | Linux after | Chrome reference |
|---|---|---|---|---|
| U01 | File page and relative link | PASS | PASS | PASS |
| U02 | Full HTTP address entry | PASS | PASS | PASS |
| U03 | Mouse hyperlink | PASS | PASS | PASS |
| U04 | Back | PASS | PASS | PASS |
| U05 | Forward | PASS | PASS | PASS |
| U06 | Reload | PASS | PASS | PASS |
| U07 | New tab | PASS | PASS | PASS |
| U08 | Independent tab field state | PASS | PASS | PASS |
| U09 | Close active tab | PASS | PASS | PASS |
| U10 | Solid CSS rendering | PASS | PASS | PASS |
| U11 | Unicode visible glyphs | PARTIAL | PARTIAL | PARTIAL |
| U12 | PNG image | PASS | PASS | PASS |
| U13 | JavaScript mouse event | PASS | PASS | PASS |
| U14 | Text input and editing | PASS | PASS | PASS |
| U15 | Required validation | PASS | PASS | PASS |
| U16 | GET form submission | PASS | PASS | PASS |
| U17 | POST form submission | PASS | PASS | PASS |
| U18 | Select keyboard control | FAIL | PASS | PASS |
| U19 | Timer after click | PASS | PASS | PASS |
| U20 | Fetch after click | PASS | PASS | PASS |
| U21 | HTTP302 redirect | PASS | PASS | PASS |
| U22 | HTTP404 feedback | PASS | PASS | PASS |
| U23 | Recovery after unavailable URL | PASS | PASS | PASS |
| U24 | Chinese operating-system IME composition | BLOCKED | BLOCKED | BLOCKED |
| U25 | Checkbox mouse and keyboard | PASS | PASS | PASS |
| U26 | Scroll and activate bottom link | PASS | PASS | PASS |
