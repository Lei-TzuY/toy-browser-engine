# v0.1.0 Research Preview release readiness

Experimental engine/research platform, not a production browser or commercial-browser
compatibility claim. Use controlled/trusted content only. Master tracker:
[#286](https://github.com/Lei-TzuY/toy-browser-engine/issues/286).

## Evidence baseline and candidate identity

Starting main: `181f4a10287701e3615c4406f7acd24c0efe5ed4`.
Its [Ubuntu CI](https://github.com/Lei-TzuY/toy-browser-engine/actions/runs/40817136278)
passes 1,893 tests. Fresh local locked builds/tests measure 1,893 Linux and 1,888
native Windows tests; platform-conditional coverage differs.

The candidate PR names its exact HEAD and changed files. Package `BUILD.json`,
archive receipts and tracker comments bind builds, executed binaries, hashes,
GUI checks and CI to that candidate. Preliminary baseline results are not silently
promoted to final candidate evidence.

## Verified behavior, bounded by fixtures

| Area | Reproduction / boundary |
| --- | --- |
| Parsing/rendering/CLI | Built-in demo renders valid 800×600 PPM; help and loopback HTTP error behavior are measured. |
| HTTPS | `tests/https.rs`: trusted private-CA positives, public-root rejection, hostname rejection, truncated/unclean TLS and HTTPS dispatch. No TLS bypass. |
| Session security | Cookie/HSTS, mixed content, referrer, CORS/preflight, integrity and redirect tests cover particular boundaries, not an audit. |
| Forbidden script headers | Real-socket tests cover Cookie/Cookie2, mixed case, reserved prefixes, method overrides and preservation of network-generated headers. |
| Redirects | 35 status/method combinations, body/header retention/cleanup, mixed chains, budgets and cross-origin credential stripping. |
| Linux X11 GUI | Real Xvfb windows: tabs, editing, exact queued input, held shortcuts, Unicode and clean exit. Receipts give counts/binary SHA. |
| Native Windows GUI | Own-window scan-code SendInput in controlled US English: launch, tabs, address navigation, ordinary input, staggered releases and slow-load exact text. Compilation alone is not GUI evidence. |

Final results belong to the exact candidate's receipts, including any failures.
The harness does not replace the production adapter with a model.

## Known limitations

- No verified OS/renderer sandbox, site isolation or safe arbitrary-web boundary.
  The single process runs with the user's privileges. Rust tests/memory safety
  do not establish browser security.
- Non-standards-complete JavaScript, CSS, DOM, URL handling and modern Web APIs.
  No compatibility certification, large-site benchmark or replacement claim for
  Chrome/Edge/Brave/Firefox. Long-term ambition is separate from this milestone.
- IME composition, clipboard, undo, selection and bidirectional editing are
  incomplete. Microsoft Bopomofo did not deliver the initial Latin-address fixture;
  US English is the bounded Windows smoke target, not general IME/layout correctness.
- Live Wayland GUI, native macOS, accessibility, multi-monitor/high-DPI behavior,
  broad keyboard layouts and long-running stability are unverified. WSL2/X11 is
  not evidence for every Linux desktop/distribution.
- HTTP/2, HTTP/3, full streaming/incremental rendering, a production persistent
  profile, password manager, extensions and automatic updates are absent or outside
  this preview. Unsigned binaries have no signing/reputation guarantee.
- Synchronous loads/long callbacks can delay frames. Resource budgets, lifecycle
  leaks and engine-specific interpreter behavior remain research limitations.
- No full independent dependency/advisory/security audit is claimed. A lockfile
  and TLS fixtures do not replace an audit.

## Experimental functionality and integration boundaries

Cache policy/Vary/age/revalidation and Fetch Metadata contain tested primitives.
A complete browser HTTP cache and universal metadata emission are not established;
the current Browser Fetch path does not emit every Sec-Fetch field.
Integrity-Policy/Reporting and COEP/CORP contain tested policy/loader components,
not a full CSP/site-isolation system. Mutation observation and other APIs may have
partial implementations; symbols do not prove complete default-Browser behavior.

Cookies, HSTS and localStorage are independent per tab, not a shared production
privacy profile. Fixtures use synthetic pages, loopback and test CA material,
not unknown public sites or real credentials.

## Security assumptions

Use a disposable development context and controlled content. Local navigation
can read files available to the process. Script file-containment/policy tests
do not provide OS-level filesystem isolation. Do not weaken host protections to
run an unsigned downloaded executable.

## Release gates and blockers

Publish only when the candidate's receipt index confirms:

1. Exact candidate SHA/tree, clean tracked checkout and one focused PR.
2. Locked Linux and native Windows builds, full tests and release binaries.
3. Actual native GUI/keyboard checks for shipped targets and clean exit.
4. Archive extraction, content hashes, extracted executable smoke, SHA256SUMS
   and complete license/dependency notices for both packages.
5. Exact-head Ubuntu/Windows CI, explicitly separated from GUI evidence.
6. Independent Reviewer C (separate session/agent) assessment of that SHA and
   actual receipts, with no unresolved release blocker.

Missing native evidence, licensing, artifact integrity, essential security
statements or an independent verdict means **RELEASE_HOLD**. Unsupported features
above remain explicit preview limits; they cannot be relabeled as tested/safe.
Only a passing candidate may become an experimental/pre-release GitHub Release
titled **v0.1.0 Research Preview**. A draft/local archive is not publication;
verify the actual Release URL and uploaded asset hashes.

## Historical integration debt

Umbrellas #267, #238, #185 and #194 are merged. The initial 185 open PRs are a
historical stack, not 185 missing features. [pr-inventory.json](pr-inventory.json)
records ancestry, patches, dependencies, conflicts and uncertainty. Three integrated
PRs without explicit base dependents are closed in the first bounded batch;
all source branches are preserved. Other proposals remain open pending evidence.
