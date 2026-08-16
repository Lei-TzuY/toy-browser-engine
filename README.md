# Toy Browser Engine

A browser engine written from scratch in Rust: it loads a page and its
subresources, parses HTML and CSS, runs JavaScript against a live DOM, keeps an
event loop turning over timers, microtasks, promises, `fetch()` and animation
frames, lays the result out, paints it to pixels, and follows links.

```powershell
cargo run                     # render the built-in demo site
cargo run -- --window         # open it in an interactive window
```

## Architecture

```text
  URL ──► net::ResourceLoader ──────────────► bytes
           (file / http / in-memory)
                    │
                    ▼
  document::Document::load
    │  html::parse_html          → dom::Node tree
    │  <base href>               → base URL for every relative reference
    │  <style> + <link rel=…>    → css::parse_css, in document order
    │  <script src> + inline     → script::JsRuntime (one runtime per page)
    │  <img src>                 → image::ImageCache (PNG / JPEG / PPM)
                    │
                    ▼
  style::style_tree_full   → StyledNode tree (cascade, inheritance, @media)
  layout::layout_tree      → LayoutBox tree  (block, inline, flex, grid, table)
  paint::build_display_list→ DisplayCommand list → Canvas → PPM / window
                    │
                    ▼
  browser::Browser         → history, link activation, back / forward / reload

  input (window / test) ──► Browser ──► Document
        KeyEvent                        │  dispatch keydown / click (DOM event)
        click                           │  default action, unless cancelled
                                        │  edit value / toggle / move focus / submit
                                        ▼
                              style → layout → paint (next frame)

  one frame of Browser::tick()
    1. platform input           (window loop)
    2. DOM events + defaults    (click, keydown, …)  → microtask checkpoint
    3. finished requests        settle their fetch() promises
                                → a microtask checkpoint after EACH completion
    4. new requests             handed to the network, nothing waited for
    5. due timer callbacks      setTimeout / setInterval
                                → a microtask checkpoint after EACH callback
    6. animation-frame callbacks  requestAnimationFrame, one shared timestamp
                                → a microtask checkpoint after EACH callback
    7. final microtask checkpoint; DOM and style now settled
    8. render → present pixels
```

A *task* is a timer, a frame request or a network completion; a *microtask* is a
promise reaction or a `queueMicrotask` callback. Steps 3 and 5 drain the
microtask queue between callbacks rather than after all of them, which is what
makes promise ordering correct.

Collecting answers (3) before sending requests (4) is deliberate: it means a
request started during one turn can never complete before the next one, however
fast its source is. That is the same rule the scheduler applies to a timer
registered mid-turn, and it is what makes `fetch()` observably asynchronous even
against a resource already in memory.

| Module | Responsibility |
| --- | --- |
| [`net`](src/net/) | `Url` (RFC 3986 resolution), `ResourceLoader` trait, file/HTTP/in-memory loaders |
| [`net::fetch`](src/net/fetch.rs) | `Method`, `HeaderMap`, `Origin`, request/response/error model, the network backends and the in-flight registry — no JavaScript anywhere in it |
| [`html`](src/html/) | Tokenizer (raw text, RCDATA, character references) and tree builder (implied end tags) |
| [`css`](src/css/) | Selector, value and at-rule parsing, shorthand expansion |
| [`style`](src/style/) | Cascade, specificity, `!important`, inheritance, `@media`, `:hover` |
| [`layout`](src/layout/) | Box tree and formatting contexts |
| [`paint`](src/paint/) | Display list, rasterizer, PPM export |
| [`script`](src/script/) | JavaScript lexer, parser, interpreter and DOM API |
| [`script::promise`](src/script/promise.rs) | Promise state, settlement and the resolution procedure, with no dependency on the interpreter |
| [`script::host`](src/script/host.rs) | `Headers`, `Request`, `Response`, `AbortController` — the objects a page holds |
| [`script::fetch_api`](src/script/fetch_api.rs) | `fetch()`: arguments in, pending promise out; and the completion that settles it |
| [`script::json`](src/script/json.rs) | JSON parsing and serialising, shared by `JSON.parse` and `response.json()` |
| [`image`](src/image.rs) | Image decoding and the per-document cache |
| [`document`](src/document.rs) | One loaded page: DOM + stylesheet + runtime + images |
| [`browser`](src/browser.rs) | Session: navigation, history, click and key routing |
| [`input`](src/input.rs) | Platform-independent `Key`, `Modifiers`, `KeyEvent` |
| [`forms`](src/forms.rs) | Focusability, tab order, successful controls, GET serialization |
| [`editing`](src/editing.rs) | Caret and value editing commands for text controls |
| [`eventloop`](src/eventloop.rs) | `Clock`, timer queue, animation-frame queue, microtask queue, ordering and budgets |
| [`platform`](src/platform.rs) | minifb → `KeyEvent` adapter (the only toolkit-aware file) |

`main.rs` is only a CLI shell: argument parsing, tree dumps and the window loop.

## Resource loading

Everything the engine fetches goes through the `ResourceLoader` trait, so the
pipeline never touches a socket or the filesystem directly.

| Scheme | Support |
| --- | --- |
| `file:` | Full read access; CLI paths are converted to `file://` URLs |
| `http:` | Built-in HTTP/1.1 client (redirects, chunked encoding, status handling, timeouts) |
| `https:` | **Not supported** — reported as a typed error rather than downgraded (no TLS stack) |
| in-memory | `MemoryLoader`, used by the embedded demo site and by tests |

URL handling covers absolute URLs, protocol-relative `//host/p`, root-relative
`/p`, relative `p` and `../p`, `?query`, `#fragment`, percent-encoding, IPv6
hosts, and `<base href>`.

Subresources are loaded through the same path:

- **CSS** — `<link rel="stylesheet">` and `<style>` enter the cascade in
  document order, under the UA stylesheet and above nothing else.
- **JavaScript** — `<script src>` and inline scripts run in document order in a
  single persistent `JsRuntime`, so state declared in one file is visible to the
  next and survives every later event.
- **Images** — fetched, sniffed by signature and decoded into RGBA; failures are
  cached so a broken URL is not retried every frame.

A subresource that fails to load never fails the page: it is recorded in
`Document::diagnostics` and the page still renders.

This is the *bootstrap* path and it is synchronous — it runs before there is a
page to be asynchronous on behalf of. What a script asks for later goes through
[Fetch](#fetch) instead, which is asynchronous end to end. The two share the
layer underneath: one `Url`, one `ResourceLoader`, one HTTP client.

## Supported features

| Area | Support |
| --- | --- |
| HTML | Raw text (`<script>`, `<style>`), RCDATA (`<textarea>`, `<title>`), character references, void elements, implied end tags (`<li>`, `<p>`, `<tr>`, `<td>`, `<dt>`, `<option>`, table sections), comments, DOCTYPE |
| CSS selectors | Tag, `#id`, `.class`, `[attr=value]`, descendant, child, `+`, `~`, `:hover`/`:active`, `:nth-child`/`:nth-of-type`, `:not`, `:first-child`, `:empty`, `:root`, … |
| CSS cascade | Specificity, source order, `!important`, inline `style=""`, inheritance, custom properties with `var()`, `@media (min/max-width)` |
| Box model | Margins (with collapsing), padding, borders, `box-sizing`, `min`/`max-width`, `margin: auto`, `position: relative/absolute/fixed`, `z-index` stacking, `overflow: hidden` |
| Layout modes | Block, inline (line breaking, `text-align`, baseline-aligned inline-blocks), **flex** (direction incl. reversed, wrap, gap, `flex-basis`/`flex`, grow/shrink, `justify-content`, `align-items`, `align-self`), **grid** (`fr`/`px`/`%` tracks, gaps, row stretching), **table** (content-proportional columns, row stretching) |
| Images | PNG, JPEG and PPM; intrinsic sizing, `width`/`height` attributes, CSS sizing, aspect-ratio preservation, `object-fit: fill/contain/cover`, alt-text fallback |
| Paint | Solid and rounded backgrounds, linear gradients, borders, alpha-blended box shadows, opacity, clipping, bilinear-sampled bitmaps, text with underline/strikethrough |
| JavaScript | `let`/`const`/`var`, `if`/`else`, `while`, `for`, `for…of`, `break`/`continue`, functions, closures, arrow functions, `new`, `throw`, `try`/`catch`/`finally`, arrays, objects, template-free strings, `typeof`, `??`-free operators, `console`, `Math`, string/array methods |
| DOM API | `getElementById`, `querySelector(All)`, `createElement`, `appendChild`, `remove`, `textContent`, `innerHTML`, `setAttribute`, `classList`, `style.<prop>`, `children`, `parentElement`, `addEventListener`, bubbling, `event.target`, `preventDefault`, `stopPropagation` |
| Navigation | `<a href>` activation with relative resolution, fragment navigation without reload, history back/forward, reload, URL/title/history shown in the window title |
| Async | Task queue (`setTimeout`, `setInterval`, `requestAnimationFrame` and their cancels), a separate microtask queue with real checkpoints, `queueMicrotask`, and a full `Promise` — `new Promise`, `then`/`catch`/`finally`, chaining and adoption, `resolve`/`reject`/`all`/`race`/`allSettled`/`any` |
| Network | `fetch(url \| Request, init)` with `method`/`headers`/`body`/`signal`, `Request`, `Response` (`status`, `ok`, `url`, `redirected`, `text()`, `json()`), case-insensitive `Headers`, `AbortController`, same-origin enforcement, GET/HEAD/POST/PUT/DELETE/PATCH over HTTP/1.1 with redirects |

### The JavaScript runtime is long-lived

```html
<button id="btn">Clicks: 0</button>
<script src="js/app.js"></script>
```

```js
let clicks = 0;                                   // declared once, at load
document.getElementById("btn").addEventListener("click", function () {
    clicks++;                                     // still here on every click
    document.getElementById("btn").textContent = "Clicks: " + clicks;
});
```

Runaway scripts are contained rather than fatal: call depth and loop iterations
are capped, and a script that trips a limit is abandoned with a console message
while the page keeps rendering.

## Focus, keyboard and forms

### Focus model

The document owns focus, keyed by a stable element identity rather than a DOM
path, so inserting nodes before the focused element cannot move focus somewhere
else.

| Behaviour | Support |
| --- | --- |
| Focusable | `<input>` (not `hidden`), `<textarea>`, `<select>`, `<button>`, `<a href>`, anything with `tabindex`; disabled controls are excluded |
| Tab order | positive `tabindex` first in ascending order, then document order; `tabindex="-1"` is focusable but not tabbable; Tab wraps |
| Moving focus | click (walks up to the nearest focusable ancestor), Tab, Shift+Tab, `element.focus()`, `element.blur()` |
| Reading focus | `document.activeElement` (the `<body>` when nothing is focused) |
| Events | `blur` and `focusout` on the old element, then `focus` and `focusin` on the new one. `focus`/`blur` do **not** bubble; `focusin`/`focusout` do |
| CSS | `:focus` and `:focus-within` resolve during selector matching, not by script |

### Keyboard pipeline

```text
platform key  →  Browser::key_down
              →  Document dispatches `keydown` (bubbling, cancelable)
              →  default action, unless preventDefault() was called
                   • Tab / Shift+Tab   move focus
                   • printable char    insert at the caret  → `input`
                   • Backspace/Delete  edit                 → `input`
                   • arrows/Home/End   move the caret
                   • Space             toggle a checkbox or radio → `input`, `change`
                   • Enter             activate a button, or submit a form → `submit`
              →  Browser::key_up dispatches `keyup`
```

Event objects expose `key`, `code`, `shiftKey`, `ctrlKey`, `altKey`, `target`,
`currentTarget`, `type`, `bubbles`, `preventDefault()` and `stopPropagation()`.

### Form controls

| Control | Support |
| --- | --- |
| `<input type=text>` (and search/url/tel/email/password/number) | live value separate from the `value` attribute, placeholder, `maxlength`, `readonly`, `disabled`, caret movement, click-to-place caret, typing, Backspace/Delete, Home/End |
| `<textarea>` | initial value from its content, multi-line editing, Enter inserts a newline, line-wise Home/End, up/down caret movement keeping its column |
| `<input type=checkbox>` | `checked` attribute as the default, click and Space toggle, `input` + `change` |
| `<input type=radio>` | same-name group exclusion within the owning form, click and Space select |
| `<button>` / `<input type=submit\|reset>` | submit and reset default actions |
| `<form>` | `submit` event, GET submission, `form.elements`, `input.form`, `form.submit()` (no submit event, as in the DOM), `form.reset()` |

Submission collects the *successful controls* — named, enabled, and checked
where applicable — and serialises them into the query string:

```html
<form action="/search" method="get">
  <input name="q" value="toy browser">
  <input name="off" value="x" disabled>
  <input type="checkbox" name="exact">
</form>
```

```text
/search?q=toy+browser
```

Disabled controls, unchecked boxes and radios, and unnamed controls are left
out; spaces become `+` and everything else is percent-encoded.

### DOM properties added

`value`, `defaultValue`, `checked`, `defaultChecked`, `disabled`, `readOnly`,
`placeholder`, `name`, `type`, `selectionStart`/`selectionEnd`, `form`,
`elements`, plus the methods `focus()`, `blur()`, `submit()` and `reset()`.

### CSS pseudo-classes

`:focus`, `:focus-within`, `:checked`, `:disabled`, `:enabled`,
`:placeholder-shown`, `:hover`, `:active`, and the structural ones
(`:nth-child`, `:not`, …). All of them run in real selector matching, so they
participate in the cascade like any other selector.

## Event loop, timers and animation

Scripts do not only run at load: the page keeps a timer queue and an
animation-frame queue, and the browser turns them over once per frame.

### Where the pieces live

| Piece | Responsibility |
| --- | --- |
| [`eventloop::Clock`](src/eventloop.rs) | Monotonic milliseconds. `RealClock` in a window, `ManualClock` in tests |
| [`eventloop::Scheduler<T>`](src/eventloop.rs) | The queues and the ordering rules, generic over the callback type so they can be tested without JavaScript |
| `JsRuntime` | Owns the scheduler for its page and hands out task ids; it never reads a clock — loop time is injected before every callback |
| `Document::run_event_loop` | Runs due timers, then animation frames, then applies whatever the callbacks asked for |
| `Browser::tick` / `advance_time` | Owns the clock, drives one turn, and carries out navigations a callback requested |

### Supported APIs

| API | Support |
| --- | --- |
| `setTimeout(fn, delay)` | function expressions, arrow functions and closures; missing, negative or `NaN` delays clamp to 0 |
| `clearTimeout(id)` | cancels a pending timer and releases its callback |
| `setInterval(fn, delay)` | repeats; delays below 4ms are clamped so a page cannot spin the loop |
| `clearInterval(id)` | cancels, including from inside the interval's own callback |
| `requestAnimationFrame(fn)` | one shot, called with the frame timestamp |
| `cancelAnimationFrame(id)` | cancels a pending frame request |
| `performance.now()` | milliseconds since this document loaded, from the loop clock |
| `Date.now()` | the same loop time (not a calendar date) |

`setTimeout("code string", …)` is **not** supported — the first argument must
be a function.

### Ordering rules

- Timers fire once `now >= deadline`, earliest deadline first.
- Equal deadlines fire in registration order, and a repeating interval keeps
  its original place in that order.
- A timer scheduled *while* callbacks are running never runs in the same turn:
  nested `setTimeout(…, 0)` becomes a task for the next turn, so the loop
  cannot be starved by re-entrancy.
- A late interval fires **once** and is rescheduled from now — it never tries
  to catch up on missed periods.
- Each turn runs at most 64 callbacks; the rest wait for the next turn.
- Animation-frame callbacks all receive the same timestamp, run in
  registration order, and a frame requested from inside a frame runs on the
  *next* one.

### Errors and lifetime

A callback that throws — `nonexistent.foo()` — reports a `TypeError` to the
console and the loop carries on: other timers still fire and the page still
renders.

Cancelling a task drops its callback, and navigating away or reloading drops
the whole document, taking its scheduler, its callbacks and their captured
scopes with it. There is no back/forward cache, so returning to a page starts
it over with a fresh runtime.

### Deterministic testing

Timer tests never sleep. They hand the session a `ManualClock` and step it:

```rust
let clock = Rc::new(ManualClock::new());
let mut browser = Browser::open_with_clock(loader, &url, clock)?;

browser.advance_time(Duration::from_millis(100));          // one turn
browser.advance_time_in_steps(                              // frame-by-frame
    Duration::from_secs(2),
    Duration::from_millis(16),
);
```

`Browser::next_wakeup_in_ms()` reports when the page next needs attention, so a
driver can idle instead of spinning.

## Microtasks and Promises

A timer is a **task**. A promise reaction is a **microtask**. They live in two
different queues with two different rules, and that difference is the whole
reason `Promise` behaves the way it does.

| | Task queue | Microtask queue |
| --- | --- | --- |
| Holds | `setTimeout`, `setInterval`, `requestAnimationFrame` | promise reactions, `queueMicrotask` |
| Ordered by | deadline, then registration | strict FIFO |
| Runs | at most 64 per turn, then the loop moves on | **to exhaustion**, including anything queued while draining |
| Where | [`eventloop::Scheduler<T>`](src/eventloop.rs) | [`eventloop::MicrotaskQueue<T>`](src/eventloop.rs) |

Draining the microtask queue is a **checkpoint**. The document runs one:

- after the page's scripts have run, before the first frame is ever rendered;
- after **each** timer or interval callback;
- after **each** animation-frame callback;
- after dispatching an event to its listeners;
- once more at the end of every loop turn.

That per-callback placement is what makes this the real model rather than an
approximation: a microtask queued by a timer runs **before the next timer**,
not after all of them.

```javascript
setTimeout(function () {
    log("timer");
    queueMicrotask(function () { log("micro"); });
}, 0);
setTimeout(function () { log("next timer"); }, 0);
// timer -> micro -> next timer
```

A checkpoint runs to exhaustion, so a chain settles completely before the loop
moves on. The only limit is an anti-starvation budget of
`MAX_MICROTASKS_PER_CHECKPOINT` (1024): a microtask that endlessly re-queues
itself is cut off with a console note and the remainder runs at the next
checkpoint, so the page degrades instead of hanging. Draining is a loop, never
recursion, so a thousand-deep promise chain costs no Rust stack.

### Promise

`Promise` is a real value in the runtime: `JsValue::Promise` wrapping an
`Rc<RefCell<PromiseState>>` from [`src/script/promise.rs`](src/script/promise.rs).
It is not a string tag, an object with a magic property, or a DOM side effect.
The state machine is the specified one, **pending -> fulfilled** or **pending ->
rejected**, once and permanently.

| API | Support |
| --- | --- |
| `new Promise(executor)` | the executor runs **synchronously**; a throw inside it rejects the promise |
| `resolve(v)` / `reject(r)` | the two functions handed to the executor; later calls are ignored |
| `.then(onFulfilled, onRejected)` | handlers are **never** called synchronously, even on an already-settled promise |
| `.catch(f)` | exactly `then(undefined, f)` |
| `.finally(f)` | runs either way and passes the original outcome through, unless `f` itself throws |
| `Promise.resolve(v)` | a promise passes through unchanged; anything else is wrapped |
| `Promise.reject(r)` | an already-rejected promise |
| `Promise.all(list)` | fulfils with results **in input order**; the first rejection rejects the whole |
| `Promise.race(list)` | the first settlement of either kind wins |
| `Promise.allSettled(list)` | never rejects; each entry becomes `{ status, value }` or `{ status, reason }` |
| `Promise.any(list)` | the first fulfilment wins; rejects only if every entry rejected |
| `queueMicrotask(fn)` | shares one FIFO with promise reactions |

`new` is a general expression form rather than a special case for `Promise`:
the parser produces `Expr::New { callee, args }` for any constructor, and
constructing something that is not one throws a `TypeError`.

Chaining follows the resolution procedure, so all of this works:

```javascript
Promise.resolve(1)
    .then(function (x) { return x + 1; })          // value passthrough
    .then(function () { throw "boom"; })           // a throw becomes a rejection
    .then(function (v) { return v; })              // skipped: no rejection handler
    .catch(function (r) { return "recovered"; })   // ...caught here
    .then(function () {
        return new Promise(function (resolve) {
            setTimeout(function () { resolve("waited"); }, 300);
        });
    })
    .then(function (v) { console.log(v); });       // adoption: waits for the inner promise
```

Returning a promise from a handler **adopts** it, so the chain waits. Resolving
a promise with itself is detected and rejects with a `TypeError` instead of
recursing forever. Several `.then()`s on one promise all fire; a handler is not
consumed by the first registration.

### Exceptions

Promises needed real exceptions, so the interpreter grew them properly rather
than by special-casing. `throw` and `try` / `catch` / `finally` (with the catch
binding optional) parse and run, and a `Flow::Throw` unwinds through
statements, calls and expression operands until something catches it.

Runtime type errors are ordinary throws rather than log lines, so a page can
catch them:

```javascript
try { nothing.foo(); } catch (e) { console.log("caught " + e); }
Promise.resolve()
    .then(function () { return nothing.foo(); })
    .catch(function (e) { console.log(e); });
```

An exception that escapes a callback is reported as `Uncaught (in timer) ...`,
`(in animation frame)`, `(in microtask)` or `(in event listener)`, and the loop
carries on: one broken callback never stops the others or the render.

### Promises and the rest of the browser

- A promise resolved inside a timer settles in **that timer's own checkpoint**,
  before any later timer runs.
- A promise handler may call `requestAnimationFrame`, and the frame it books
  runs on the next turn.
- Promise handlers mutate the DOM like any other script, and the change is
  painted on the next render.
- Navigating or reloading drops the document, its microtask queue and every
  pending reaction: **a promise from the old page can never run on the new
  one.** There is no back/forward cache, so returning re-runs the scripts from
  scratch.

Because settling a promise only *queues* microtasks and never runs them, no
promise operation can re-enter the interpreter mid-expression. `promise.rs`
returns a `Vec<Microtask>` from every state change and the runtime enqueues it;
the module holds no reference back to the runtime, so the two carry no cycle.

## Fetch

`fetch()` does no I/O. It validates its arguments, creates a pending promise,
records the request and returns — on the caller's stack, in constant time.
Which is why this prints A, B, C and not A, C, B, however local the resource:

```javascript
console.log("A");
fetch("/data.json").then(function () { console.log("C"); });
console.log("B");
```

### The path a request takes

```text
  fetch(input, init)                       script/fetch_api.rs
    → FetchRequest                         net/fetch.rs   (plain Send data)
    → FetchRegistry                        the page's in-flight list; owns the promise
        ↓ (next turn)
    → NetworkBackend::start                net/fetch.rs
        ↓ local read, or a worker thread
    → FetchCompletion                      bytes, status, headers — nothing else
        ↓ (a later turn: a TASK)
    → JsRuntime::settle_fetch              resolve the promise with a Response
        ↓ microtask checkpoint
    → .then(response => response.json())   a MICROTASK
        ↓ microtask checkpoint
    → .then(data => …)                     DOM mutation
        ↓
    → render
```

Three layers, each with one job. `net::fetch` knows HTTP and not JavaScript.
`script::host` knows JavaScript and not sockets. The registry in between owns
the pending promise and is the sole authority on whether an answer is still
wanted — which is the whole navigation story: dropping a document drops its
registry, drops its promises, and a late answer settles nothing.

### Backends

| Backend | Used for | How it avoids blocking |
| --- | --- | --- |
| `LocalNetwork` | `file:` and the in-memory demo site | Reads during the loop's network phase — outside any JavaScript stack |
| `ThreadedNetwork` | `http:` | A worker thread per request; the answer comes back down a channel |
| `DefaultNetwork` | production | Routes by scheme, exactly as `DefaultLoader` does |
| `ManualNetwork` | tests and drivers | Completes only when told to |
| `OfflineNetwork` | a document with no session | Fails every request with a clear message |

A worker owns an `Arc<dyn ResourceLoader>` and a `FetchRequest` — both `Send`,
neither connected to the DOM, the runtime or a promise, and the `Send` bound is
what enforces that. It cannot settle a promise, paint, or touch a node; its only
output is bytes. There is no `unsafe` anywhere in the path.

### Response

`fetch()` resolves for **any** response, including a 404 or a 500. Only a
failure to *get* an answer rejects.

```javascript
fetch("/missing").then(function (r) {
    r.status;      // 404
    r.ok;          // false — 2xx and nothing else
    r.statusText;  // "Not Found"
    r.url;         // the final URL, after redirects
    r.redirected;  // true if any redirect was followed
    r.headers;     // a live Headers
    r.bodyUsed;    // false, until something reads it
});
```

`response.text()` and `response.json()` both return promises, even though the
bytes are already in memory — reading a body is never synchronous. Each may be
called **once**: the second reads a body that is gone and rejects with a
`TypeError`, and `bodyUsed` flips to true. `text()` decodes UTF-8, dropping a
byte-order mark and replacing invalid sequences; `json()` rejects with a
`SyntaxError` naming the position that failed.

### Request, Headers, AbortController

```javascript
const request = new Request("/api/save", {
    method: "post",                                    // normalised to POST
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name: "toy" })
});
request.url; request.method; request.headers; request.bodyUsed;
fetch(request);                                        // sending does not consume the body
```

`Headers` is case-insensitive and multi-valued: `get` joins duplicates with
`", "`, `set` replaces them, `append` adds. `new Headers(object)` and
`new Headers(otherHeaders)` both work, and `Request.headers` and
`Response.headers` are the same abstraction — there is one header
implementation in the engine, not two.

`GET` and `HEAD` may not carry a body. `GET`, `HEAD`, `POST`, `PUT`, `DELETE`
and `PATCH` are sent; anything else rejects rather than being passed through.

```javascript
const controller = new AbortController();
fetch("/slow", { signal: controller.signal }).catch(function (e) { /* AbortError */ });
controller.abort();
```

Aborting rejects every request watching that signal, un-sends anything still in
the outbox and tells the backend to drop what is already on the wire. A socket
in flight is not interrupted — but its answer never reaches the page, and the
promise and request state are released immediately. Aborting before the fetch
means it never starts; aborting after it completes does nothing.

### Origins

The engine enforces same-origin and does not implement CORS. There is no
preflight, no `Access-Control-Allow-Origin` handling, and no opaque response.

| Document | May fetch |
| --- | --- |
| `http://host:port/…` | the same scheme, host and port, and nothing else |
| `file:` / `demo:` | the same scheme, confined to its own directory subtree |

So a local page can read a sibling `api/data.json` but not `../secrets.txt`,
and a page served over HTTP can never read `file:///etc/passwd`. Real browsers
treat `file:` as opaque; the directory rule here is a deliberate simplification
that keeps the fixture site usable while still bounding what a page can reach.

`mode` accepts `"cors"` and `"same-origin"` — both enforced as same-origin,
because there is no preflight to make `cors` mean more — and rejects anything
else rather than pretending. `credentials` accepts `"same-origin"` and
`"omit"`; `"include"` is rejected, because there are **no cookies at all** in
this engine: no jar, no `Set-Cookie`, no `document.cookie`.

### Errors

These, and only these, reject:

| Rejection | When |
| --- | --- |
| invalid URL | it cannot be resolved against the document |
| unsupported scheme | anything outside `http:`/`file:`/the document's own — including `https:`, which has no TLS stack |
| blocked | the same-origin policy said no |
| bad request | a body on a `GET`, an unusable header, an unsupported `mode` |
| unsupported method | `TRACE`, `CONNECT`, anything not in the list above |
| I/O, timeout, malformed response, too many redirects | the network |
| `AbortError` | `controller.abort()` |
| too many requests | past `MAX_IN_FLIGHT_FETCHES` |

Every one arrives as a rejected promise, never as a synchronous throw, so
`.catch()` sees all of them. Reasons are readable strings rather than `Error`
objects — this engine has no error classes yet.

### Limits

A page is allowed `MAX_IN_FLIGHT_FETCHES` (6) requests at once; past that,
`fetch()` rejects immediately rather than building a backlog, so a runaway loop
fails visibly instead of consuming memory. Response bodies are capped at 32MB,
header blocks at 64KB, and redirect chains at 5 hops. Scripts cannot set `Host`,
`Content-Length`, `Connection` or the other headers the engine owns, and a
header value containing a newline is refused outright.

There is **no HTTP cache**: two `fetch()` calls for one URL are two requests.

### Navigation and subresources

Documents, stylesheets, scripts and images still load synchronously when a page
is navigated to — that is the bootstrap, and it happens before there is a page
to be asynchronous on behalf of. What both paths share is the layer underneath:
one `Url`, one `ResourceLoader`, one HTTP client. `http::send` is the whole
client and speaks the fetch vocabulary; `http::get` is navigation's view of the
same code, differing only in that a page which 404s cannot be rendered while a
`fetch()` that 404s is a perfectly good answer.

## CLI

```text
browser_engine [options] [<url-or-file> [<out.ppm>]]

  <url-or-file>   file path, file:// or http:// URL (default: built-in demo site)
  <out.ppm>       write the rendered page here (default: output.ppm)

  --window        interactive window
  --size WxH      canvas and viewport size (default 800x600)
  --quiet         skip the DOM and layout tree dumps
  --help          usage
```

In the window, keystrokes go to the **page** — click a field and type. Browser
chrome is on `Alt`:

| Key | Action |
| --- | --- |
| click | focus a control, follow a link, press a button |
| typing, Backspace, Delete, arrows, Home, End | edit the focused control |
| `Tab` / `Shift+Tab` | move focus |
| `Space` | toggle the focused checkbox or radio |
| `Enter` | submit a form, or activate a button |
| mouse wheel | scroll |
| — | timers and animations run on their own, at 60fps |
| `Alt`+`←` / `Alt`+`→` | back / forward |
| `Alt`+`R` | reload |
| `Alt`+`↑` / `Alt`+`↓` | scroll by a page step |
| `Esc` | quit |

Examples:

```powershell
cargo run -- examples/site/index.html            # render a local page
cargo run -- examples/site/index.html page.ppm   # …and save it
cargo run -- --window --size 1000x800 http://localhost:8000/
```

## The example site

`examples/site/` is a real multi-page site used both as the built-in demo
(embedded at compile time and served from memory) and as the fixture for the
end-to-end tests:

```text
examples/site/
├── index.html          external CSS + JS, flex, grid, table, images, links
├── form.html           text input, textarea, checkboxes, radios, GET submit
├── async.html          setTimeout, setInterval, requestAnimationFrame, cancel
├── promise.html        task/microtask order, chaining, rejection, Promise.all
├── fetch.html          GET JSON/text, 404, POST, Promise.all, a blocked origin
├── api/data.json       what the fetch page loads and renders
├── api/note.txt        …and reads with response.text()
├── results.html        where the form submission lands
├── pages/about.html    second document, all references via ../
├── css/site.css        external stylesheet
├── css/form.css        :focus, :checked, :disabled styling
├── js/app.js           external script (click counter, DOM mutation)
├── js/form.js          external script (focus/input/change/submit listeners)
├── js/async.js         external script (timers, animation loop, cancellation)
├── js/promise.js       external script (ordering log, chains, Promise.all)
├── js/fetch.js         external script (requests, responses, bodies, abort)
├── logo.png            PNG with alpha
├── photo.jpg           JPEG
└── assets/icon.png     PNG in a subdirectory
```

Both routes produce pixel-identical output, which is what the loader
abstraction is for.

To drive the form page headlessly — focus, typing, Tab, Space, submit — and
write the result to an image:

```powershell
cargo run --example interact -- interact.ppm
cargo run -- --window examples/site/form.html   # …or do it by hand
```

To watch the timers and the animation on virtual time, writing one image per
sampled moment (0, 100, 500, 1000, 2000 and 5000 ms):

```powershell
cargo run --example async_driver -- frames
cargo run -- --window examples/site/async.html  # …or watch it live
```

To watch the promise page, which prints the execution order it actually
observed and writes a frame at 0, 50, 300, 600 and 1000 ms:

```powershell
cargo run --example promise_driver -- frames
cargo run -- --window examples/site/promise.html
```

Its first sample needs no time at all: the whole microtask log is already
written before the clock moves, which is the point.

To watch a fetch, with a network that answers only when told to:

```powershell
cargo run --example fetch_driver -- frames
cargo run -- --window examples/site/fetch.html   # …or click through it live
```

It writes `fetch_0` (idle), `fetch_pending` (the request is on the wire and the
promise has not settled), `fetch_done` (parsed and rendered) and `fetch_all`.
The middle frame is the one a real network could not give you reliably.

## Test

```powershell
cargo test
```

Current verification: **731 tests** — 653 unit tests plus 78 integration tests
in `tests/`. They load the fixture site from disk, check the cascade, run its
scripts, verify image decoding and sizing, click a rendered button, follow a
link, walk history, serve the same site over a loopback HTTP server, drive the
form page through focus, typing, Tab, checkbox toggling and a GET submission,
run the timers-and-animation page on virtual time, step the promise page
through its microtask checkpoints, and drive the fetch page through a network
that answers only on command — asserting the repaint changes each time.

Each async layer is tested at every level: the queue and registry types on
their own (FIFO order, drain-to-exhaustion, budgets, in-flight limits, `Rc`
release), the promise state machine and the HTTP parser without any
interpreter, the runtime through JavaScript source, and the whole browser
through the fixture page.

`tests/network.rs` is the other half: eighteen tests against a real HTTP server
on `127.0.0.1`, covering GET, POST with a body, HEAD, custom headers, redirect
chains, a redirect loop, 404 and 500, a refused connection, a malformed
response, a cancelled request, and a page that fetches JSON over a socket and
renders it. The server binds to port 0 and reports the port it was given, so
the client always connects to a listener that is already accepting; the two
threads meet at `join()`.

No test touches the public network, and **no test sleeps**: timing tests
advance a `ManualClock` by hand, and the socket tests synchronise on a thread
handle or a channel. The one place the engine waits on the network is
`Browser::settle_network`, and that is a channel readiness wait that returns
the instant data arrives — not a fixed delay.

Quality gates, all clean:

```powershell
cargo fmt --check           # no diff
cargo test --locked         # 731 passing, 0 failed, 0 ignored
cargo clippy --all-targets  # 0 warnings
```

## Known limitations

- **No TLS**, so `https:` URLs are refused with a clear error.
- **No streaming or incremental rendering**: a document is fully loaded, then
  laid out.
- There is no module system and no network API for scripts, and `Math.random`
  is deterministic to keep renders reproducible.
- Element handles are DOM paths, so restructuring the tree can stale a handle a
  script is holding.
- Layout omits floats, multi-column, writing modes, `vertical-align` beyond the
  baseline default, and `grid-template-areas`.
- Images are decoded eagerly and never re-fetched; there is no HTTP cache.
- The window has no editable address bar — the title bar shows URL, title and
  history position.
- **Forms**: `POST` is recognised but refused rather than half-implemented (the
  loader only issues GETs); there is no `<select>` dropdown UI (the element is
  focusable and submits its `value`), no file inputs, no `multipart/form-data`,
  no constraint validation (`required`, `pattern`) and no `formaction`.
- **Editing**: there is no text selection, clipboard, undo, IME or
  bidirectional text; a `<textarea>` clips rather than soft-wrapping long
  lines, and a long value in an `<input>` is clipped rather than scrolled.
- **Keyboard**: `event.code` is a simplification of the physical key, key
  repeat comes from the platform, and `keypress`/`beforeinput` are not fired.
- **Async**: timers, microtasks, promises and `fetch` are implemented;
  `async`/`await` syntax, `XMLHttpRequest`, `WebSocket`, `EventSource`,
  workers, service workers, `MutationObserver`, CSS transitions and CSS
  animations are not. Everything a page can observe runs on the single browser
  thread — the only other thread is the one doing blocking socket reads, and it
  can touch nothing but bytes — so a callback that runs long delays the next
  frame rather than being pre-empted.
- **Fetch**: no streaming (`ReadableStream`, `response.body`), no
  `FormData`/`Blob`/`ArrayBuffer` bodies (a body is a string), no `response.clone()`,
  no `Cache`, no HTTP cache, no cookies, no CORS preflight or
  `Access-Control-*` handling, no `Referer`, no `keepalive`, no upload or
  download progress, and no timeout in the Fetch API itself (the HTTP client
  has one). Combinators take arrays, not iterables. `AbortController` cannot
  interrupt a socket already in flight; it drops the answer instead.
- **Promises**: only arrays are accepted by the combinators (there is no
  iterator protocol), rejection reasons are plain values rather than `Error`
  objects with stacks, `thenable` objects are not assimilated (only real
  promises are), and an unhandled rejection is reported at the point it
  escapes rather than tracked to the end of the turn.
- **Memory**: there is no garbage collector. A function stored in the scope it
  closes over (a plain top-level `function`) forms a reference cycle that is
  not collected; cancelling a timer or dropping a document does release
  everything the scheduler held.
- minifb reports characters and physical keys separately, so the adapter in
  `platform.rs` merges the two streams; character input therefore follows the
  OS keyboard layout, while named keys are mapped by the adapter.

This is an educational engine, not a production browser: the goal is to keep the
whole pipeline inspectable and testable rather than standards-complete.
