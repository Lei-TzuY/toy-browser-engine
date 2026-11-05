# Trusted local browser usability scenarios

This suite executes 26 authored scenarios through actual browser windows or a reference browser's input interface. It uses only generated local files and an owned loopback server. The corpus is a reproducible usability sample; it does not establish safety or compatibility on arbitrary websites.

`usability_corpus.py` owns the exact 19 HTML files, PNG bytes, dynamic routes and case descriptions. HTML is written as UTF-8 bytes so Linux and Windows receive identical files. `usability_suite.py` performs the interactions and records actual window captions, committed URLs, server requests, screenshots and pixel checks. It never treats text remaining in script source as an executed JavaScript result.

Run a release build: the window backend samples some input once per frame, so debug rendering can miss short input holds. Every run needs a fresh evidence directory. Results include the exact source commit, executable hash, fixture hashes, runner hashes, raw actions, exceptions and case-by-case observations. Keep failed runs when correcting a harness or product problem.

For Linux, provide Xvfb, xdotool, ImageMagick's `import`, and Pillow through the system or a private tool prefix. No existing desktop is used: the adapter starts its own Xvfb display, one owned browser child per scenario, checks window/pointer ownership before input, and closes its processes afterwards.

```sh
CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 cargo build --release --locked
python3 tests/gui/usability/linux_usability.py \
  target/release/browser_engine /tmp/toy-usability-linux-unique \
  --source-repo "$PWD" --tool-prefix /path/to/private/prefix
```

`--cases U07 U18` selects a bounded supplement. `--font /path/to/font.ttf` sets the primary font; `--fallback-font /path/to/font.ttf` supplies `BROWSER_ENGINE_FALLBACK_FONT` for glyphs the primary lacks. Effective paths and configured-font hashes are recorded separately from the default environment. Do not silently use them to turn a default-font limitation into a pass. The product also tries a fixed list of installed OS fallback fonts lazily; no font is installed or redistributed by this harness.

The native Windows adapter operates one owned HWND and restores its input layout. Its screenshot method must capture only that HWND's client, never a desktop rectangle. The reference adapter uses a fresh Chrome profile with loopback fixtures, actual CDP input events, live title/URL reads and page screenshots. Reference navigation/tab APIs and headless rendering are recorded as capabilities; they do not certify native browser chrome or operating-system IME.

For CLI rendering corroboration only:

```sh
python3 tests/gui/usability/headless_render.py \
  target/release/browser_engine /tmp/toy-usability-render-unique \
  --source-repo "$PWD" --pillow-prefix /path/to/private/prefix
```

That mode actually renders U10 (solid CSS), U11 (Unicode glyph capture) and U12 (PNG). The other 23 scenarios remain `NOT_TESTED`: loading a page without its required input does not execute an interaction scenario.

The result statuses have distinct meanings:

| Status | Meaning |
|---|---|
| PASS | The required action ran and its observable checks passed. |
| PARTIAL | Required evidence or a capability remains incomplete; retain the actual observations. |
| FAIL | An executed action or observable expectation failed. |
| BLOCKED | A required capability is unavailable. |
| NOT_TESTED | The mode did not execute this scenario. |

U11 starts `PARTIAL` until a human reviews the actual glyph pixels against the actual reference capture. A Unicode title or DOM value cannot establish visible font coverage. Record missing Chinese glyphs explicitly. U24 requires real operating-system composition; committed Unicode injection is a different capability and cannot make it pass.

See [ADAPTER_CONTRACT.md](ADAPTER_CONTRACT.md) for the adapter API, exact coordinates, request schema and unavailable-port reservation.
