# Toy Browser Engine — v0.1.0 Research Preview

An independent Rust browser engine and reproducible research platform. It loads
controlled HTML/CSS/JavaScript, runs an inspectable interpreter and event loop,
lays out a document, paints pixels, and offers an experimental multi-tab window.

**Use trusted demos and controlled fixtures. This is not a production browser or
an endorsement for browsing arbitrary websites.** The process is not protected
by a verified renderer/OS sandbox. Passing Rust tests does not certify security
or compatibility with Chrome, Edge, Brave, Firefox, or real-world web apps.

The long-term ambition is a credible independent browser. The current milestone
is a useful open-source engine, technical demonstration and research platform;
commercial-browser claims require future compatibility, security and performance
evidence. See [release readiness](docs/release-readiness.md) and
[release notes](docs/release-notes-0.1.0.md).

## Build and run

Use a clean checkout and the committed lockfile. Linux x86-64 and native Windows
x86-64 are the Research Preview targets. WSL runs the Linux target; it does not
constitute Windows-native testing. macOS and live Wayland GUI behavior are not
release-verified. See [platform dependencies and installation](docs/build-and-run.md).

```sh
cargo build --locked
cargo test --locked
cargo build --release --locked
cargo run --locked --release -- --quiet --screenshot demo.ppm
cargo run --locked --release -- --window --quiet
```

The first screenshot/window uses the built-in demo and needs no public network.
On Windows the release executable is `target/release/browser_engine.exe`;
on Linux it is `target/release/browser_engine`. Run from a writable directory.
The binaries are unsigned; no installer or auto-update service is provided.
Do not disable host security controls to run them.

## Verified surfaces and evidence

These are bounded implementations, not declarations of complete standards support.

| Surface | Evidence / scope |
| --- | --- |
| HTML/CSS/layout/paint | Parser, cascade, block/inline/flex/grid/table and image tests; [fixture site](examples/site/) and PPM output. Modern site compatibility is unmeasured. |
| JavaScript/DOM/event loop | In-process interpreter, live DOM, events, timers, promises and microtask checkpoints; deterministic tests in [tests/](tests/). No full ECMAScript/DOM certification or JIT. |
| Browser session/network | Verified TLS, mixed-content checks, cookies, HSTS, referrer policy and browser-owned redirect/CORS handling have loopback and Browser-level tests. See [HTTPS](tests/https.rs), [session network](tests/session_network.rs), [mixed content](tests/mixed_content.rs). |
| Fetch | Request/response guards, forbidden script headers, simple CORS and preflight, redirect modes, body cloning and integrity checks have targeted tests. [Real sockets](tests/network.rs), [request guards](tests/fetch_request_headers_guard.rs), [method overrides](tests/fetch_method_override_header_guard.rs), [redirect security](tests/http_redirect_security.rs). |
| Forms | GET and URL-encoded POST, select state, validation and submitter overrides have Browser tests. [POST](tests/form_post.rs), [validation](tests/form_validation.rs), [submitter](tests/form_submitter_browser.rs). This is not a complete native form-control UI. |
| Tabs and keyboard | Per-tab document/history/scroll/address/session state. [X11 window suite](tests/gui/tabs_window.py), [shortcut routing](tests/gui/shortcut_routing.py), [native Windows smoke](tests/gui/windows_smoke.py). |
| Experimental policy components | HTTP cache/revalidation, Fetch Metadata, Integrity-Policy/Reporting and COEP/CORP contain tested components. API presence alone does not establish complete default-Browser wiring. Full browser caching and site isolation are not release guarantees. |

At baseline `181f4a10287701e3615c4406f7acd24c0efe5ed4`, clean locked tests passed:
**1,893 on Linux and 1,888 on native Windows**. Platform-conditional tests differ.
Exact candidate build, GUI, archive and CI receipts accompany the release-candidate
PR and release assets; counts are tied to a commit, not a permanent project promise.

`cargo fmt --check` and Clippy have inherited formatting/warning debt. Do not
interpret an old zero-warning/731-test claim as current evidence. The M1 work
compares diagnostics against exact main rather than reformatting unrelated code.

## Window controls

The tab strip sits above a toolbar with Back, Forward, Reload and an address
editor. Type explicit `http://`, `https://`, or `file://` URLs, or a local path.
A bare hostname is currently treated as a local path.

| Input | Action |
| --- | --- |
| Ctrl+T / + | New tab at the start page; address selected |
| Ctrl+Tab / Ctrl+Shift+Tab | Next / previous tab |
| Ctrl+1…8 / Ctrl+9 | Tab by position / last tab |
| Ctrl+W / tab × / middle click | Close tab; closing the last tab closes the window |
| Ctrl+L / address click | Edit address; Enter navigates, Escape cancels |
| Alt+Left / Alt+Right / Alt+R | Back / Forward / Reload |
| Tab / Shift+Tab | Move document focus |
| Typing, arrows, Home/End, Backspace/Delete | Basic editing, with platform limitations below |
| Escape outside editing | Close the window |

Only the active tab receives input and is painted. Tabs keep independent cookies,
HSTS and localStorage state rather than a shared persistent browser profile.
Tabs cannot be dragged, reopened or created below the strip's minimum width;
Ctrl+W remains available when a close icon cannot fit.

Native callbacks carry physical source codes for shortcut ownership, including
unknown text sources. The pinned minifb patch is documented in
[vendor/minifb/PATCH.md](vendor/minifb/PATCH.md). Layout, IME and clipboard behavior
still require platform-specific validation; the patch is not a general IME fix.

## Architecture

`ResourceLoader` supplies bytes to HTML/CSS/script/image loading. The engine
builds a DOM and style tree, computes layout, creates a display list and paints
PPM pixels or a minifb window. `Browser` owns navigation/history; a tab adds its
own toolbar and scroll state. Bootstrap loading is synchronous. Later Fetch
requests use a worker-backed network and settle promises through the browser
loop; this is not process isolation.

A browser turn routes input, settles network completions, starts queued requests,
runs timers and animation frames, and drains microtasks at checkpoints. Tests
use a manual clock or loopback synchronization. GUI tests deliberately wait for
OS frames/events; the historical claim that no tests sleep is no longer accurate.

## CLI

```text
browser_engine [options] [<url-or-file> [<out.ppm>]]
  --window        tabs and toolbar
  --size WxH      viewport dimensions (default 800x600)
  --viewport WxH  alias for --size
  --screenshot P  write a PPM image
  --quiet         omit tree dumps
  --stats         performance/node diagnostics
  --json-stats    structured metrics
  --inspect       DOM and computed style details
  --dump-html     serialize DOM
  --dump-css      active stylesheets
  --benchmark     pipeline stress/timing experiment
  --help          full command help
```

Built-in demo pages also cover forms, timers, promises and Fetch. Headless driver
examples are available in [examples/](examples/). Benchmarks are experiments,
not comparisons against commercial browsers.

## Known limitations and security assumptions

- The application and renderer share one process. No verified OS/renderer sandbox,
  site isolation or safe arbitrary-content boundary is provided. Local navigation
  can access files available to the user account; script file-containment tests
  do not replace OS isolation.
- JavaScript, CSS, DOM and URL handling are incomplete. Modules, async/await,
  WebSocket/XMLHttpRequest, service workers and many modern Web APIs are not
  compatibility targets. Existing APIs may implement only a subset of a standard.
- IME composition, selection, clipboard, undo and bidirectional editing are
  incomplete. Native Windows smoke requires a controlled US English layout;
  the host's Microsoft Bopomofo mode did not deliver the tested Latin address
  sequence. No working Chinese/Japanese/Korean IME workflow is promised.
- Live Wayland GUI, macOS GUI, accessibility, high-DPI/multi-monitor behavior and
  broad keyboard-layout compatibility are not release-verified. X11 GUI and
  Windows synthetic keyboard fixtures are narrower evidence than real-user coverage.
- HTTP/1.1 and verified TLS 1.2/1.3 are implemented; HTTP/2, HTTP/3 and a complete
  production network stack are not. No insecure TLS fallback is offered.
- Mixed-content, cookie, HSTS, CORS, referrer, redirect and integrity tests cover
  specific boundaries. They are not a complete browser threat model or security audit.
- No verified complete browser HTTP cache, persistent profile, password manager,
  extension platform, signing or automatic updates. Storage/session isolation is
  experimental and not a privacy guarantee.
- No streaming/incremental rendering, full Web Streams/body types, or complete
  modern form controls. Synchronous loads and long callbacks can delay a frame.
- Memory/lifecycle management is not a production garbage collector; resource
  budgets and cycle/lifetime behavior remain research topics.

## Development and release evidence

```sh
cargo test --locked
cargo clippy --locked --all-targets
cargo fmt --check
```

The CI labels separate Ubuntu tests, Windows-native tests and actual X11 GUI
smoke. Windows CI compilation/CLI smoke is not Windows GUI evidence. For native
GUI reproduction and package verification, follow [build-and-run](docs/build-and-run.md).

The [PR inventory](docs/pr-inventory.json) records exact historical heads, ancestry,
merge checks, dependencies and confidence. An open PR is not a missing feature.
[STABILIZATION.md](STABILIZATION.md) records completed integrations and M1 policy.

MIT-licensed project code. Downloadable packages include the root license,
locked dependency license texts/notices and Rust toolchain copyright notices.
No fonts from the host operating system are redistributed.
