// ============================================================
//  document.rs  —  Loading a page and everything it references
// ============================================================
//
//  A `Document` is one loaded page: its DOM, the stylesheet the cascade
//  should use, the JavaScript runtime that ran its scripts, and the images it
//  referenced. Building one is the "navigate" half of a browser:
//
//    1. fetch the HTML and parse it
//    2. resolve the base URL (`<base href>` overrides the document URL)
//    3. collect the stylesheet: UA rules, then `<style>` and
//       `<link rel=stylesheet>` in document order
//    4. run `<script>` elements in document order — inline and external —
//       through a single, persistent runtime
//    5. fetch and decode the images the resulting DOM references
//
//  Everything is fetched through a `ResourceLoader`, so the same code path
//  serves `file:`, `http:` and in-memory documents.

use std::rc::Rc;

use crate::css::parser::{parse_css, Stylesheet};
use crate::dom::{ElementData, ElementId, Node, NodeType};
use crate::editing::{self, EditCommand, EditResult};
use crate::forms::{self, Submission};
use crate::image::{ImageCache, RasterImage};
use crate::input::{FocusDirection, Key, KeyEvent};
use crate::layout::{layout_tree_with, LayoutBox};
use crate::net::{LoadError, NetworkBackend, OfflineNetwork, ResourceLoader, Url};
use crate::paint::{paint_with_scroll, Canvas};
use crate::script::interp::{EventInit, EventOutcome, JsValue, PendingAction};
use crate::script::{dom_api, JsRuntime, NodePath};
use crate::style::{style_tree_full, InteractionState, StyledNode};

/// The user-agent stylesheet: the browser's own defaults, the weakest layer of
/// the cascade.
pub const UA_STYLESHEET: &str = r#"
html, body { display: block; }
div, p, h1, h2, h3, h4, h5, h6,
ul, ol, li, dl, dt, dd, blockquote, pre,
header, footer, section, article, nav, main, aside,
form, fieldset, table, thead, tbody, tfoot, tr, td, th,
figure, figcaption { display: block; }

span, a, strong, em, b, i, u, s, code, abbr, cite,
small, sub, sup, label, button, input, img { display: inline-block; }

head, script, style, meta, link, title, noscript, base { display: none; }

h1 { font-size: 32px; }
h2 { font-size: 24px; }
h3 { font-size: 18px; }
p  { margin: 16px 0; }
ul, ol { margin: 16px 0; padding-left: 40px; }
ul { list-style-type: disc; }
ol { list-style-type: decimal; }
a  { text-decoration: underline; color: #0000ee; cursor: pointer; }
button { padding: 8px 16px; border-radius: 4px; border: none; cursor: pointer; }
"#;

/// Something that went wrong while loading a subresource. A page with a broken
/// stylesheet still renders, so these are collected rather than propagated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub url: String,
    pub message: String,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.url, self.message)
    }
}

/// What the browser must do after the document handled an event.
///
/// The document can change its own DOM, but navigating is the session's job,
/// so anything that leaves the page comes back out as one of these.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PageAction {
    #[default]
    None,
    Submit(Submission),
}

/// What one turn of the event loop did.
#[derive(Debug, Default)]
pub struct LoopReport {
    /// Timer callbacks that ran.
    pub timers_run: usize,
    /// Animation-frame callbacks that ran.
    pub frames_run: usize,
    /// Finished requests whose promises were settled.
    pub network_completions: usize,
    /// Requests handed to the network this turn.
    pub requests_sent: usize,
    /// A submission a callback asked for through `form.submit()`.
    pub submission: Option<Submission>,
}

impl LoopReport {
    /// True when a callback ran, so the page may look different now.
    pub fn did_work(&self) -> bool {
        self.timers_run > 0 || self.frames_run > 0 || self.network_completions > 0
    }
}

/// Geometry of a laid-out control, used to place the caret from a click.
struct ControlBox {
    content_x: f32,
    content_y: f32,
    font_size: f32,
}

/// Build the event-object fields a keyboard event carries.
fn key_event_init(event: &KeyEvent) -> EventInit {
    EventInit::bubbling()
        .with_field("key", JsValue::Str(event.key.key_value()))
        .with_field("code", JsValue::Str(event.key.code_value()))
        .with_field("shiftKey", JsValue::Bool(event.modifiers.shift))
        .with_field("ctrlKey", JsValue::Bool(event.modifiers.ctrl))
        .with_field("altKey", JsValue::Bool(event.modifiers.alt))
}

/// Translate a key press into an editing command for a text control.
fn edit_command_for(event: &KeyEvent, element: &ElementData) -> Option<EditCommand> {
    if !element.is_text_entry() {
        return None;
    }
    if let Some(character) = event.typed_character() {
        return Some(EditCommand::Insert(character));
    }
    Some(match event.key {
        Key::Enter => EditCommand::InsertNewline,
        Key::Backspace => EditCommand::Backspace,
        Key::Delete => EditCommand::Delete,
        Key::ArrowLeft => EditCommand::MoveLeft,
        Key::ArrowRight => EditCommand::MoveRight,
        Key::ArrowUp => EditCommand::MoveUp,
        Key::ArrowDown => EditCommand::MoveDown,
        Key::Home => EditCommand::MoveToStart,
        Key::End => EditCommand::MoveToEnd,
        _ => return None,
    })
}

/// What the pointer is over, addressed by DOM path so it survives re-layout.
#[derive(Debug, Default, Clone)]
pub struct PointerState {
    pub hovered: Option<NodePath>,
    pub active: Option<NodePath>,
    pub focused: Option<NodePath>,
}

/// One loaded page.
pub struct Document {
    /// The URL this document was loaded from (after redirects).
    pub url: Url,
    /// Base for resolving relative references — `<base href>` or `url`.
    pub base_url: Url,
    pub dom: Node,
    /// UA rules followed by author rules in document order.
    pub stylesheet: Stylesheet,
    /// The runtime that ran this page's scripts; kept alive so its globals and
    /// listeners are still there when events fire.
    pub runtime: JsRuntime,
    pub images: ImageCache,
    /// Subresources that failed to load or decode.
    pub diagnostics: Vec<Diagnostic>,
    /// The focused element, kept as a stable id so DOM mutations cannot
    /// silently move focus to a different element.
    focus: Option<ElementId>,
    /// A submission requested from a callback, waiting for the session to
    /// navigate. Only the session can leave the page, so it is parked here.
    deferred_submission: Option<Submission>,
    /// Where this page's `fetch()` calls go.
    ///
    /// Held rather than borrowed per call, so dropping the document drops its
    /// view of the network along with its pending promises. Until a session
    /// attaches a real one, a page is offline and every request fails with a
    /// clear message instead of silently doing nothing.
    network: Rc<dyn NetworkBackend>,
    /// Active CSS transitions for elements in the document.
    pub transitions: std::cell::RefCell<crate::transition::TransitionManager>,
    /// Active CSS animations for elements in the document.
    pub animations: std::cell::RefCell<crate::animation::AnimationManager>,
}

impl Document {
    /// Fetch `url` and everything it references.
    pub fn load(url: &Url, loader: &dyn ResourceLoader) -> Result<Document, LoadError> {
        Document::load_with_storage(url, loader, None)
    }

    /// Fetch `url` and everything it references, providing initial persistent storage.
    pub fn load_with_storage(
        url: &Url,
        loader: &dyn ResourceLoader,
        storage: Option<crate::script::interp::StorageRef>,
    ) -> Result<Document, LoadError> {
        let resource = loader.load(url)?;
        Ok(Document::from_html_with_storage(&resource.text(), &resource.url, loader, storage))
    }

    /// Build a document from HTML that has already been fetched.
    ///
    /// `url` is used both as the document's address and as the base for
    /// relative references (until a `<base href>` says otherwise).
    pub fn from_html(html: &str, url: &Url, loader: &dyn ResourceLoader) -> Document {
        Document::from_html_with_storage(html, url, loader, None)
    }

    /// Build a document from HTML with caller-supplied persistent storage.
    pub fn from_html_with_storage(
        html: &str,
        url: &Url,
        loader: &dyn ResourceLoader,
        storage: Option<crate::script::interp::StorageRef>,
    ) -> Document {
        let mut dom = crate::html::parse_html(html);
        let base_url = base_url_of(&dom, url);
        let mut diagnostics = Vec::new();

        // A textarea's initial value is its text content, not an attribute.
        seed_textarea_values(&mut dom);

        let stylesheet = collect_stylesheet(&dom, &base_url, loader, &mut diagnostics);
        let referrer_policy = crate::referrer_meta::apply_meta_referrer_policies(
            &dom,
            crate::referrer_policy::ReferrerPolicy::default(),
        );
        let runtime = run_scripts(
            &mut dom,
            &base_url,
            url,
            referrer_policy,
            loader,
            &mut diagnostics,
            storage,
        );

        let mut document = Document {
            url: url.clone(),
            base_url,
            dom,
            stylesheet,
            runtime,
            images: ImageCache::new(),
            diagnostics,
            focus: None,
            deferred_submission: None,
            network: Rc::new(OfflineNetwork::new()),
            transitions: std::cell::RefCell::new(crate::transition::TransitionManager::new()),
            animations: std::cell::RefCell::new(crate::animation::AnimationManager::new()),
        };
        // Scripts have finished: run the microtasks they queued, so a
        // `Promise.resolve().then(…)` at load time lands before the first
        // paint rather than waiting for a timer.
        document.run_microtask_checkpoint();
        document.refresh_images(loader);
        document
    }

    /// Fetch and decode any `<img>` sources that are not cached yet.
    ///
    /// Called after loading and after scripts mutate the DOM, so images added
    /// at runtime are painted too.
    pub fn refresh_images(&mut self, loader: &dyn ResourceLoader) {
        for source in image_sources(&self.dom) {
            let Ok(url) = self.base_url.join(&source) else {
                self.diagnostics.push(Diagnostic {
                    url: source.clone(),
                    message: "could not resolve image URL".into(),
                });
                continue;
            };
            if self.images.get(&url).is_some() || self.images.error(&url).is_some() {
                continue;
            }
            if let Err(message) = self.images.fetch(&url, loader) {
                self.diagnostics.push(Diagnostic {
                    url: url.to_string(),
                    message,
                });
            }
        }
    }

    /// Resolve a reference (an `href`, `src`, …) against this document's base.
    pub fn resolve(&self, reference: &str) -> Option<Url> {
        self.base_url.join(reference).ok()
    }

    /// Look up the decoded image for an element's `src`.
    fn image_for(&self, element: &ElementData) -> Option<Rc<RasterImage>> {
        let source = element.get_attr("src")?;
        let url = self.base_url.join(source).ok()?;
        self.images.get(&url)
    }

    /// Style the DOM for a viewport width and pointer state.
    pub fn style_tree(&self, viewport_width: f32, pointer: &PointerState) -> StyledNode<'_> {
        let interaction = InteractionState {
            hovered_node: pointer
                .hovered
                .as_ref()
                .and_then(|p| dom_api::node_at(&self.dom, p)),
            active_node: pointer
                .active
                .as_ref()
                .and_then(|p| dom_api::node_at(&self.dom, p)),
            // The document owns focus; a caller may still override it (the
            // renderer uses this to preview a focus ring).
            focused_node: pointer
                .focused
                .clone()
                .or_else(|| self.focused_path())
                .and_then(|p| dom_api::node_at(&self.dom, &p)),
        };
        let mut styled = style_tree_full(&self.dom, &self.stylesheet, viewport_width, &interaction);
        self.animations.borrow_mut().update_and_apply(&mut styled, &self.stylesheet, self.runtime.now_ms);
        self.transitions.borrow_mut().update_and_apply(&mut styled, self.runtime.now_ms);
        styled
    }

    /// Lay the document out. The styled tree must outlive the returned boxes,
    /// so callers keep it alive themselves.
    pub fn layout<'a>(&'a self, styled: &'a StyledNode<'a>, viewport_width: f32) -> LayoutBox<'a> {
        layout_tree_with(
            styled,
            viewport_width,
            &|element: &ElementData| self.image_for(element),
            self.focus,
        )
    }

    /// Style, lay out and paint in one step.
    pub fn render(
        &self,
        width: usize,
        height: usize,
        scroll_y: f32,
        pointer: &PointerState,
    ) -> Canvas {
        let styled = self.style_tree(width as f32, pointer);
        let layout = self.layout(&styled, width as f32);
        paint_with_scroll(&layout, width, height, 0.0, scroll_y)
    }

    /// Total laid-out height of the page, for scroll clamping.
    pub fn content_height(&self, viewport_width: f32) -> f32 {
        let styled = self.style_tree(viewport_width, &PointerState::default());
        let layout = self.layout(&styled, viewport_width);
        layout.dimensions.margin_box().height
    }

    /// The DOM path of the node at a point in page coordinates.
    pub fn hit_test(&self, x: f32, y: f32, viewport_width: f32) -> Option<NodePath> {
        let styled = self.style_tree(viewport_width, &PointerState::default());
        let layout = self.layout(&styled, viewport_width);
        let node = layout.hit_test(x, y)?;
        dom_api::path_of(&self.dom, node)
    }

    /// Walk up from `path` to the nearest `<a href>` and resolve its target.
    pub fn link_at(&self, path: &[usize]) -> Option<Url> {
        for ancestor in dom_api::ancestor_paths(path) {
            let node = dom_api::node_at(&self.dom, &ancestor)?;
            if let NodeType::Element(element) = &node.node_type {
                if element.tag_name == "a" {
                    let href = element.get_attr("href")?;
                    return self.resolve(href);
                }
            }
        }
        None
    }

    /// The document's `<title>`, if it has one.
    pub fn title(&self) -> Option<String> {
        let path = dom_api::query_selector(&self.dom, &[], "title")?;
        let text = dom_api::text_content(dom_api::node_at(&self.dom, &path)?);
        let trimmed = text.trim().to_string();
        (!trimmed.is_empty()).then_some(trimmed)
    }

    // ── Focus ─────────────────────────────────────────────────────────────

    /// The focused element's stable identity, if anything has focus.
    pub fn active_element(&self) -> Option<ElementId> {
        self.focus
    }

    /// Where the focused element currently sits in the tree.
    pub fn focused_path(&self) -> Option<NodePath> {
        dom_api::path_of_element_id(&self.dom, self.focus?)
    }

    /// Move focus to `path`, firing the four focus events in DOM order:
    /// `blur` and `focusout` on the old element, then `focus` and `focusin` on
    /// the new one. `focus`/`blur` do not bubble; `focusin`/`focusout` do.
    ///
    /// Returns false when the element cannot take focus.
    pub fn focus_path(&mut self, path: &[usize]) -> bool {
        let Some(element) = dom_api::node_at(&self.dom, path).and_then(|n| n.as_element()) else {
            return false;
        };
        if !forms::is_focusable(element) {
            return false;
        }
        let target = element.element_id();
        if self.focus == Some(target) {
            return true;
        }

        self.blur_focused();
        self.focus = Some(target);
        self.runtime.focused = self.focus;

        let path = path.to_vec();
        self.dispatch(&path, "focus", EventInit::non_bubbling());
        self.dispatch(&path, "focusin", EventInit::bubbling());
        true
    }

    /// Remove focus from whatever has it, firing `blur` and `focusout`.
    pub fn blur_focused(&mut self) {
        let Some(path) = self.focused_path() else {
            self.focus = None;
            self.runtime.focused = None;
            return;
        };
        self.focus = None;
        self.runtime.focused = None;
        self.dispatch(&path, "blur", EventInit::non_bubbling());
        self.dispatch(&path, "focusout", EventInit::bubbling());
    }

    /// Move focus to the next (or previous) tabbable element, wrapping around.
    pub fn move_focus(&mut self, direction: FocusDirection) -> bool {
        let order = forms::tab_order(&self.dom);
        if order.is_empty() {
            return false;
        }
        let current = self
            .focused_path()
            .and_then(|path| order.iter().position(|candidate| *candidate == path));

        let next = match (current, direction) {
            (Some(index), FocusDirection::Forward) => (index + 1) % order.len(),
            (Some(index), FocusDirection::Backward) => (index + order.len() - 1) % order.len(),
            // Nothing focused yet: Tab starts at the first, Shift+Tab at the last.
            (None, FocusDirection::Forward) => 0,
            (None, FocusDirection::Backward) => order.len() - 1,
        };
        let path = order[next].clone();
        self.focus_path(&path)
    }

    /// Focus the nearest focusable ancestor of a clicked node, or clear focus.
    ///
    /// Clicking a label or plain text inside a control focuses the control,
    /// which is why this walks up rather than testing only the hit node.
    pub fn focus_from_click(&mut self, path: &[usize]) {
        for candidate in dom_api::ancestor_paths(path) {
            let focusable = dom_api::node_at(&self.dom, &candidate)
                .and_then(|n| n.as_element())
                .is_some_and(forms::is_focusable);
            if focusable {
                self.focus_path(&candidate);
                return;
            }
        }
        self.blur_focused();
    }

    // ── Events ────────────────────────────────────────────────────────────

    /// Dispatch an event at `path` and apply anything scripts asked for.
    fn dispatch(&mut self, path: &[usize], event_type: &str, init: EventInit) -> EventOutcome {
        let outcome = self
            .runtime
            .dispatch_event_init(&mut self.dom, path, event_type, init);
        self.apply_pending_actions();
        // Listeners are callbacks too: their microtasks run before the next task.
        self.run_microtask_checkpoint();
        outcome
    }

    // ── Event loop ────────────────────────────────────────────────────────

    /// Run one turn of this document's event loop at `now_ms`.
    ///
    /// The order is fixed and is what the frame lifecycle depends on:
    ///
    ///   1. finished requests, each followed by a microtask checkpoint
    ///   2. requests the page asked for, handed to the network
    ///   3. due timer callbacks (`setTimeout`, `setInterval`)
    ///   4. animation-frame callbacks, all sharing one timestamp
    ///
    /// Collecting answers *before* sending new ones is what gives the loop its
    /// invariant: a request started during turn N cannot complete before turn
    /// N+1, however fast its source is. That is the same rule the scheduler
    /// applies to a timer registered mid-turn, and it is what makes
    /// `fetch()` observably asynchronous even against a resource already
    /// sitting in memory.
    ///
    /// Every stage can mutate the DOM; the caller renders afterwards, so a
    /// change made in any of them is visible in the very next paint.
    pub fn run_event_loop(&mut self, now_ms: f64) -> LoopReport {
        // A turn starts by finishing any microtasks left from the last one.
        self.run_microtask_checkpoint();
        self.runtime.now_ms = now_ms;
        let network_completions = self.deliver_network_completions();
        let requests_sent = self.dispatch_network_requests();
        let timers_run = self.run_due_timers(now_ms);
        let frames_run = self.run_animation_frames(now_ms);
        self.run_microtask_checkpoint();
        LoopReport {
            timers_run,
            frames_run,
            network_completions,
            requests_sent,
            // Anything a callback asked the session to do, collected across
            // every stage.
            submission: self
                .apply_pending_actions()
                .or_else(|| self.deferred_submission.take()),
        }
    }

    // ── Network ───────────────────────────────────────────────────────────

    /// Point this page's `fetch()` at a network.
    ///
    /// A session does this for every document it puts on screen. The backend
    /// may be shared between documents — the registry, not the backend, is
    /// what decides whether an answer is still wanted.
    pub fn set_network(&mut self, network: Rc<dyn NetworkBackend>) {
        self.network = network;
    }

    /// Point this page's `localStorage` at a persistent origin-scoped storage pool.
    pub fn set_local_storage(&mut self, storage: crate::script::interp::StorageRef) {
        self.runtime.local_storage = storage;
    }

    /// Hand the requests scripts asked for to the network.
    ///
    /// Requests are only *started* here; nothing waits for an answer.
    pub fn dispatch_network_requests(&mut self) -> usize {
        let network = self.network.clone();
        // Anything aborted since the last turn is un-sent first, so a request
        // cancelled before it left never reaches the network at all.
        for id in self.runtime.fetches.take_cancellations() {
            network.cancel(id);
        }
        let outbox = self.runtime.fetches.take_outbox();
        let count = outbox.len();
        for (id, request) in outbox {
            network.start(id, request);
        }
        count
    }

    /// Settle the promises of requests that have finished.
    ///
    /// Each completion is a *task*: it settles one promise, and the microtask
    /// checkpoint that follows runs that promise's reactions before the next
    /// completion is delivered. A completion whose id the registry no longer
    /// knows — a late answer for the previous page — is discarded here.
    pub fn deliver_network_completions(&mut self) -> usize {
        let network = self.network.clone();
        let completions = network.poll();
        let mut delivered = 0;
        for completion in completions {
            let Some(pending) = self.runtime.fetches.take(completion.id) else {
                continue;
            };
            delivered += 1;
            self.runtime.settle_fetch(pending, completion.result);
            self.run_microtask_checkpoint();
        }
        delivered
    }

    /// True while a request is in flight or waiting to be sent.
    pub fn has_pending_network(&self) -> bool {
        self.runtime.fetches.has_pending_work() || self.network.is_busy()
    }

    /// How many requests this page is waiting on.
    pub fn in_flight_requests(&self) -> usize {
        self.runtime.fetches.len()
    }

    /// Run the timer callbacks that are due, in deadline then registration
    /// order. Callbacks scheduled while these run wait for the next turn.
    pub fn run_due_timers(&mut self, now_ms: f64) -> usize {
        self.runtime.now_ms = now_ms;
        let due = self.runtime.scheduler.take_due_timers(now_ms);
        let count = due.len();
        for (_, callback) in due {
            // Each callback is isolated: a failing one reports and the rest
            // still run.
            self.runtime
                .call_reporting(&mut self.dom, &callback, Vec::new(), "timer");
            // A microtask checkpoint follows every task, so a promise resolved
            // by this timer runs its reactions before the next timer.
            self.run_microtask_checkpoint();
        }
        if count > 0 {
            if let Some(submission) = self.apply_pending_actions() {
                self.deferred_submission = Some(submission);
            }
        }
        count
    }

    /// Run the animation-frame callbacks registered so far, handing each the
    /// same frame timestamp.
    pub fn run_animation_frames(&mut self, now_ms: f64) -> usize {
        self.runtime.now_ms = now_ms;
        let frames = self.runtime.scheduler.take_animation_frames();
        let count = frames.len();
        let timestamp = JsValue::Number(now_ms as f32);
        for (_, callback) in frames {
            self.runtime.call_reporting(
                &mut self.dom,
                &callback,
                vec![timestamp.clone()],
                "animation frame",
            );
            // Frame callbacks get a checkpoint too, so a promise resolved in
            // one is settled before the frame is painted.
            self.run_microtask_checkpoint();
        }
        if count > 0 {
            if let Some(submission) = self.apply_pending_actions() {
                self.deferred_submission = Some(submission);
            }
        }
        count
    }

    /// Run every pending microtask — a *microtask checkpoint*.
    ///
    /// This happens after each task (a script, an event listener, a timer or
    /// frame callback), which is what puts promise reactions ahead of the next
    /// task while still keeping them out of the current one.
    pub fn run_microtask_checkpoint(&mut self) -> usize {
        let ran = self.runtime.drain_microtasks(&mut self.dom);
        if ran > 0 {
            // Microtasks can focus, submit or otherwise ask the session to act.
            if let Some(submission) = self.apply_pending_actions() {
                self.deferred_submission = Some(submission);
            }
        }
        ran
    }

    /// True while promise reactions or `queueMicrotask` callbacks are waiting.
    pub fn has_pending_microtasks(&self) -> bool {
        !self.runtime.microtasks.is_empty()
    }

    /// When this document next needs a turn, in event-loop milliseconds.
    ///
    /// `Some(0.0)` means "as soon as possible" (a frame is pending); `None`
    /// means the page is idle and the driver can wait for input.
    pub fn next_wakeup_ms(&self) -> Option<f64> {
        if self.has_pending_microtasks()
            || self.runtime.scheduler.frame_count() > 0
            // A request needs a turn to be sent, and another to be collected.
            || self.has_pending_network()
        {
            return Some(0.0);
        }
        let js_deadline = self.runtime.scheduler.next_deadline_ms();
        if self.transitions.borrow().has_active() || self.animations.borrow().has_active(self.runtime.now_ms) {
            let next_frame = self.runtime.now_ms + 16.0;
            Some(match js_deadline {
                Some(d) => d.min(next_frame),
                None => next_frame,
            })
        } else {
            js_deadline
        }
    }

    /// True while any timer, frame callback or request is still outstanding.
    pub fn has_pending_tasks(&self) -> bool {
        self.runtime.scheduler.has_pending_work()
            || self.has_pending_network()
            || self.transitions.borrow().has_active()
            || self.animations.borrow().has_active(self.runtime.now_ms)
    }

    /// Drop every pending task. Navigating away from a page does this, so a
    /// departed page's intervals cannot keep running and its requests cannot
    /// settle anything: the promises go with the registry.
    pub fn cancel_all_tasks(&mut self) {
        self.animations.borrow_mut().clear();
        self.transitions.borrow_mut().clear();
        self.runtime.scheduler.clear_all();
        self.runtime.microtasks.clear();
        let network = self.network.clone();
        for id in self.runtime.fetches.clear() {
            network.cancel(id);
        }
    }

    /// Run the focus/submit/reset requests scripts queued while executing.
    ///
    /// Returns a submission if a script called `form.submit()`.
    pub fn apply_pending_actions(&mut self) -> Option<Submission> {
        let mut submission = None;
        // Handlers may queue more work, so drain until the queue settles.
        for _ in 0..8 {
            let actions = std::mem::take(&mut self.runtime.pending);
            if actions.is_empty() {
                break;
            }
            for action in actions {
                match action {
                    PendingAction::Focus(id) => {
                        if let Some(path) = dom_api::path_of_element_id(&self.dom, id) {
                            self.focus_path(&path);
                        }
                    }
                    PendingAction::Blur(id) => {
                        if self.focus == Some(id) {
                            self.blur_focused();
                        }
                    }
                    PendingAction::Submit(id) => {
                        // `form.submit()` skips the submit event by design.
                        if let Some(path) = dom_api::path_of_element_id(&self.dom, id) {
                            submission =
                                forms::prepare_submission(&self.dom, &path, &self.base_url);
                        }
                    }
                    PendingAction::Reset(id) => {
                        if let Some(path) = dom_api::path_of_element_id(&self.dom, id) {
                            self.reset_form(&path);
                        }
                    }
                    PendingAction::Reload => {}
                    PendingAction::Back => {}
                    PendingAction::Forward => {}
                }
            }
        }
        submission
    }

    // ── Keyboard ──────────────────────────────────────────────────────────

    /// Deliver a key press: `keydown`, then — unless a listener cancelled it —
    /// the default action, then `input` if the value changed.
    ///
    /// `keyup` is a separate call, because the platform reports press and
    /// release separately; [`Document::press_key`] does both for callers that
    /// only have one event.
    pub fn key_down(&mut self, event: &KeyEvent) -> PageAction {
        let target = self
            .focused_path()
            .or_else(|| dom_api::body_path(&self.dom))
            .unwrap_or_default();

        let outcome = self.dispatch(&target, "keydown", key_event_init(event));
        if outcome.default_prevented {
            return PageAction::None;
        }
        self.default_key_action(&target, event)
    }

    /// Dispatch `keyup` at the focused element. It has no default action.
    pub fn key_up(&mut self, event: &KeyEvent) {
        let target = self
            .focused_path()
            .or_else(|| dom_api::body_path(&self.dom))
            .unwrap_or_default();
        self.dispatch(&target, "keyup", key_event_init(event));
    }

    /// A complete press and release.
    pub fn press_key(&mut self, event: &KeyEvent) -> PageAction {
        let action = self.key_down(event);
        self.key_up(event);
        action
    }

    /// Type a string one character at a time, as a user would.
    pub fn type_text(&mut self, text: &str) {
        for character in text.chars() {
            self.press_key(&KeyEvent::character(character));
        }
    }

    /// The browser's own response to a key that scripts did not cancel.
    fn default_key_action(&mut self, target: &[usize], event: &KeyEvent) -> PageAction {
        // Tab moves focus wherever it is pressed.
        if event.key == Key::Tab {
            let direction = if event.modifiers.shift {
                FocusDirection::Backward
            } else {
                FocusDirection::Forward
            };
            self.move_focus(direction);
            return PageAction::None;
        }

        let Some(element) = dom_api::node_at(&self.dom, target).and_then(|n| n.as_element()) else {
            return PageAction::None;
        };

        // Space and Enter activate buttons and checkable inputs.
        if matches!(event.key, Key::Character(' ')) && element.is_checkable() {
            return self.activate(target);
        }
        if event.key == Key::Enter {
            if element.tag_name == "button" || element.is_checkable() {
                return self.activate(target);
            }
            // Implicit submission: Enter in a single-line text field submits
            // the form it belongs to.
            if element.tag_name == "input" && element.is_text_entry() {
                if let Some(form) = forms::owning_form(&self.dom, target) {
                    if forms::allows_implicit_submission(&self.dom, &form) {
                        return self.submit_form(&form);
                    }
                }
                return PageAction::None;
            }
        }

        let Some(command) = edit_command_for(event, element) else {
            return PageAction::None;
        };
        self.run_edit_command(target, command);
        PageAction::None
    }

    /// Apply an editing command and fire `input` when the value changes.
    fn run_edit_command(&mut self, target: &[usize], command: EditCommand) -> EditResult {
        let mut result = EditResult::default();
        if let Some(node) = dom_api::node_at_mut(&mut self.dom, target) {
            if let NodeType::Element(element) = &mut node.node_type {
                result = editing::apply(element, command);
            }
        }
        if result.value_changed {
            self.dispatch(target, "input", EventInit::bubbling());
        }
        result
    }

    // ── Activation ────────────────────────────────────────────────────────

    /// The default action of a click: toggle a box, submit or reset a form.
    ///
    /// The `click` event itself is dispatched by the caller, so a handler that
    /// cancels it can suppress everything here.
    pub fn activate(&mut self, target: &[usize]) -> PageAction {
        let Some(element) = dom_api::node_at(&self.dom, target).and_then(|n| n.as_element()) else {
            return PageAction::None;
        };
        if element.is_disabled() {
            return PageAction::None;
        }
        let tag = element.tag_name.clone();
        let input_type = element.input_type();

        if element.is_checkable() {
            return self.toggle_checkable(target);
        }

        let is_submit = (tag == "button"
            && element
                .get_attr("type")
                .map(|t| t.eq_ignore_ascii_case("submit"))
                .unwrap_or(true))
            || (tag == "input" && input_type == "submit");
        // `<button type=reset>` and `<input type=reset>` both reset.
        let is_reset = input_type == "reset" && (tag == "button" || tag == "input");

        if let Some(form) = forms::owning_form(&self.dom, target) {
            if is_submit {
                return self.submit_form(&form);
            }
            if is_reset {
                self.reset_form(&form);
                return PageAction::None;
            }
        }
        PageAction::None
    }

    /// Toggle a checkbox, or select a radio within its group, then fire
    /// `input` and `change`.
    fn toggle_checkable(&mut self, target: &[usize]) -> PageAction {
        let Some(element) = dom_api::node_at(&self.dom, target).and_then(|n| n.as_element()) else {
            return PageAction::None;
        };
        let is_radio = element.input_type() == "radio";
        let group = element.get_attr("name").map(str::to_string);
        let next = if is_radio {
            true
        } else {
            !element.is_checked()
        };

        // A radio deselects the rest of its group.
        if is_radio {
            for path in self.radio_group(target, group.as_deref()) {
                if path == target {
                    continue;
                }
                if let Some(node) = dom_api::node_at_mut(&mut self.dom, &path) {
                    if let NodeType::Element(other) = &mut node.node_type {
                        other.set_checked(false);
                    }
                }
            }
        }

        let changed = if let Some(node) = dom_api::node_at_mut(&mut self.dom, target) {
            match &mut node.node_type {
                NodeType::Element(element) => {
                    let before = element.is_checked();
                    element.set_checked(next);
                    before != next
                }
                _ => false,
            }
        } else {
            false
        };

        if changed {
            self.dispatch(target, "input", EventInit::bubbling());
            self.dispatch(target, "change", EventInit::bubbling());
        }
        PageAction::None
    }

    /// Radio buttons sharing a name within the same form (or document).
    fn radio_group(&self, target: &[usize], name: Option<&str>) -> Vec<NodePath> {
        let Some(name) = name else { return Vec::new() };
        let scope = forms::owning_form(&self.dom, target).unwrap_or_default();
        forms::form_controls(&self.dom, &scope)
            .into_iter()
            .filter(|path| {
                dom_api::node_at(&self.dom, path)
                    .and_then(|n| n.as_element())
                    .is_some_and(|e| e.input_type() == "radio" && e.get_attr("name") == Some(name))
            })
            .collect()
    }

    // ── Submission ────────────────────────────────────────────────────────

    /// Fire `submit` and, unless it was cancelled, prepare the navigation.
    pub fn submit_form(&mut self, form_path: &[usize]) -> PageAction {
        let outcome = self.dispatch(form_path, "submit", EventInit::bubbling());
        if outcome.default_prevented {
            return PageAction::None;
        }
        match forms::prepare_submission(&self.dom, form_path, &self.base_url) {
            Some(submission) => PageAction::Submit(submission),
            None => PageAction::None,
        }
    }

    /// Place the caret in a text control from a click position.
    ///
    /// The click's x offset inside the control's content box picks the nearest
    /// character boundary; a `<textarea>` also picks the line from y.
    pub fn place_caret_from_click(
        &mut self,
        target: &[usize],
        x: f32,
        y: f32,
        viewport_width: f32,
    ) {
        let Some(box_info) = self.control_box(target, viewport_width) else {
            return;
        };
        let Some(node) = dom_api::node_at_mut(&mut self.dom, target) else {
            return;
        };
        let NodeType::Element(element) = &mut node.node_type else {
            return;
        };
        if !element.is_text_entry() {
            return;
        }

        let value = element.control_value();
        let font_size = box_info.font_size;
        let offset_x = x - box_info.content_x;

        let caret = if element.tag_name == "textarea" {
            let line_height = crate::text::line_metrics(font_size).new_line_size;
            let line_index = (((y - box_info.content_y) / line_height).floor().max(0.0)) as usize;
            let lines = editing::value_lines(&value);
            let line_index = line_index.min(lines.len().saturating_sub(1));
            let column = editing::caret_for_offset(lines[line_index], font_size, offset_x);
            // Convert (line, column) back into an index into the whole value.
            lines[..line_index]
                .iter()
                .map(|line| line.chars().count() + 1)
                .sum::<usize>()
                + column
        } else {
            editing::caret_for_offset(&value, font_size, offset_x)
        };
        element.set_caret(caret);
    }

    /// Content-box origin and font size of a laid-out control.
    fn control_box(&self, target: &[usize], viewport_width: f32) -> Option<ControlBox> {
        let styled = self.style_tree(viewport_width, &PointerState::default());
        let layout = self.layout(&styled, viewport_width);

        fn find(layout: &LayoutBox, dom: &Node, target: &[usize]) -> Option<ControlBox> {
            if let Some(node) = layout.styled_node() {
                if dom_api::path_of(dom, node.node).as_deref() == Some(target) {
                    return Some(ControlBox {
                        content_x: layout.dimensions.content.x,
                        content_y: layout.dimensions.content.y,
                        font_size: layout.font_size(),
                    });
                }
            }
            layout.children.iter().find_map(|c| find(c, dom, target))
        }
        find(&layout, &self.dom, target)
    }

    /// Restore every control in a form to its attribute defaults.
    pub fn reset_form(&mut self, form_path: &[usize]) {
        for path in forms::form_controls(&self.dom, form_path) {
            if let Some(node) = dom_api::node_at_mut(&mut self.dom, &path) {
                if let NodeType::Element(element) = &mut node.node_type {
                    element.reset_control_value();
                    element.reset_checked();
                }
            }
        }
    }
}

// ── Loading steps ─────────────────────────────────────────────────────────────

/// `<base href>` overrides the document URL for relative references.
fn base_url_of(dom: &Node, document_url: &Url) -> Url {
    let Some(path) = dom_api::query_selector(dom, &[], "base") else {
        return document_url.clone();
    };
    let Some(node) = dom_api::node_at(dom, &path) else {
        return document_url.clone();
    };
    node.as_element()
        .and_then(|e| e.get_attr("href"))
        .and_then(|href| document_url.join(href).ok())
        .unwrap_or_else(|| document_url.clone())
}

/// Author stylesheets in document order, under the UA defaults.
fn collect_stylesheet(
    dom: &Node,
    base_url: &Url,
    loader: &dyn ResourceLoader,
    diagnostics: &mut Vec<Diagnostic>,
) -> Stylesheet {
    let mut stylesheet = parse_css(UA_STYLESHEET);

    for source in style_sources(dom) {
        match source {
            StyleSource::Inline(css) => {
                let parsed = parse_css(&css);
                stylesheet.rules.extend(parsed.rules);
                stylesheet.keyframes.extend(parsed.keyframes);
            }
            StyleSource::Link(href) => {
                let Ok(url) = base_url.join(&href) else {
                    diagnostics.push(Diagnostic {
                        url: href.clone(),
                        message: "could not resolve stylesheet URL".into(),
                    });
                    continue;
                };
                match loader.load(&url) {
                    Ok(resource) => {
                        let parsed = parse_css(&resource.text());
                        stylesheet.rules.extend(parsed.rules);
                        stylesheet.keyframes.extend(parsed.keyframes);
                    }
                    Err(error) => diagnostics.push(Diagnostic {
                        url: url.to_string(),
                        message: error.to_string(),
                    }),
                }
            }
        }
    }
    stylesheet
}

enum StyleSource {
    Inline(String),
    Link(String),
}

/// `<style>` blocks and `<link rel=stylesheet>` hrefs, in document order.
fn style_sources(dom: &Node) -> Vec<StyleSource> {
    let mut out = Vec::new();
    walk(dom, &mut out);
    return out;

    fn walk(node: &Node, out: &mut Vec<StyleSource>) {
        if let NodeType::Element(element) = &node.node_type {
            match element.tag_name.as_str() {
                "style" => {
                    let css: String = node
                        .children
                        .iter()
                        .filter_map(|c| match &c.node_type {
                            NodeType::Text(t) => Some(t.as_str()),
                            _ => None,
                        })
                        .collect();
                    out.push(StyleSource::Inline(css));
                    return;
                }
                "link" => {
                    let rel = element
                        .get_attr("rel")
                        .unwrap_or_default()
                        .to_ascii_lowercase();
                    // `rel` may list several tokens, e.g. "alternate stylesheet".
                    if rel.split_whitespace().any(|token| token == "stylesheet") {
                        if let Some(href) = element.get_attr("href") {
                            out.push(StyleSource::Link(href.to_string()));
                        }
                    }
                    return;
                }
                _ => {}
            }
        }
        for child in &node.children {
            walk(child, out);
        }
    }
}

/// Run every `<script>` in document order through one runtime.
fn run_scripts(
    dom: &mut Node,
    base_url: &Url,
    document_url: &Url,
    referrer_policy: crate::referrer_policy::ReferrerPolicy,
    loader: &dyn ResourceLoader,
    diagnostics: &mut Vec<Diagnostic>,
    storage: Option<crate::script::interp::StorageRef>,
) -> JsRuntime {
    // Sources are collected before execution: a script may restructure the DOM
    // underneath us, and the document-order list is fixed at parse time.
    let sources = script_sources(dom);
    let mut runtime = JsRuntime::new();
    runtime.url = base_url.clone();
    runtime.referrer_source = Some(document_url.clone());
    runtime.referrer_policy = referrer_policy;
    if let Some(s) = storage {
        runtime.local_storage = s;
    }

    for source in sources {
        match source {
            ScriptSource::Inline(code) => runtime.run_script(dom, &code),
            ScriptSource::External(src) => {
                let Ok(url) = base_url.join(&src) else {
                    diagnostics.push(Diagnostic {
                        url: src.clone(),
                        message: "could not resolve script URL".into(),
                    });
                    continue;
                };
                match loader.load(&url) {
                    Ok(resource) => runtime.run_script(dom, &resource.text()),
                    Err(error) => diagnostics.push(Diagnostic {
                        url: url.to_string(),
                        message: error.to_string(),
                    }),
                }
            }
        }
    }
    runtime
}

enum ScriptSource {
    Inline(String),
    External(String),
}

fn script_sources(dom: &Node) -> Vec<ScriptSource> {
    let mut out = Vec::new();
    walk(dom, &mut out);
    return out;

    fn walk(node: &Node, out: &mut Vec<ScriptSource>) {
        if let NodeType::Element(element) = &node.node_type {
            if element.tag_name == "script" {
                match element.get_attr("src") {
                    // `src` wins: the element's contents are ignored, as in HTML.
                    Some(src) if !src.trim().is_empty() => {
                        out.push(ScriptSource::External(src.to_string()))
                    }
                    _ => {
                        let code: String = node
                            .children
                            .iter()
                            .filter_map(|c| match &c.node_type {
                                NodeType::Text(t) => Some(t.as_str()),
                                _ => None,
                            })
                            .collect();
                        if !code.trim().is_empty() {
                            out.push(ScriptSource::Inline(code));
                        }
                    }
                }
                return;
            }
        }
        for child in &node.children {
            walk(child, out);
        }
    }
}

/// Copy each `<textarea>`'s text content into its live value.
///
/// HTML has no `value` attribute for textareas: the element's content is the
/// default value, so it is seeded once at parse time.
fn seed_textarea_values(dom: &mut Node) {
    fn walk(node: &mut Node) {
        let text = dom_api::text_content(node);
        if let NodeType::Element(element) = &mut node.node_type {
            if element.tag_name == "textarea" {
                element.set_control_value(text);
                element.set_caret(0);
                return;
            }
        }
        for child in &mut node.children {
            walk(child);
        }
    }
    walk(dom);
}

/// Every `<img src>` value in the tree, in document order.
fn image_sources(dom: &Node) -> Vec<String> {
    let mut out = Vec::new();
    walk(dom, &mut out);
    return out;

    fn walk(node: &Node, out: &mut Vec<String>) {
        if let NodeType::Element(element) = &node.node_type {
            if element.tag_name == "img" {
                if let Some(src) = element.get_attr("src") {
                    if !src.trim().is_empty() {
                        out.push(src.to_string());
                    }
                }
            }
        }
        for child in &node.children {
            walk(child, out);
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::parser::{Color, Value};
    use crate::input::{Key, KeyEvent, Modifiers};
    use crate::net::{ManualNetwork, MemoryLoader};

    fn site() -> MemoryLoader {
        let mut loader = MemoryLoader::new();
        loader.insert(
            "demo:///index.html",
            r#"<html><head>
                 <title>Fixture</title>
                 <link rel="stylesheet" href="css/site.css">
                 <style>.inline { color: rgb(1, 2, 3); }</style>
               </head>
               <body>
                 <p id="p" class="from-link inline">text</p>
                 <script src="js/app.js"></script>
                 <script>document.getElementById("p").setAttribute("data-inline", "ran");</script>
               </body></html>"#,
        );
        loader.insert(
            "demo:///css/site.css",
            ".from-link { background-color: rgb(9, 9, 9); color: rgb(7, 7, 7); }",
        );
        loader.insert(
            "demo:///js/app.js",
            r#"document.getElementById("p").setAttribute("data-external", "ran");"#,
        );
        loader
    }

    fn load(loader: &MemoryLoader, url: &str) -> Document {
        Document::load(&Url::parse(url).unwrap(), loader).expect("document loads")
    }

    #[test]
    fn loads_external_stylesheet_and_keeps_source_order() {
        let document = load(&site(), "demo:///index.html");
        let styled = document.style_tree(800.0, &PointerState::default());

        fn find<'a>(node: &'a StyledNode<'a>, id: &str) -> Option<&'a StyledNode<'a>> {
            if node.node.as_element().and_then(|e| e.get_attr("id")) == Some(id) {
                return Some(node);
            }
            node.children.iter().find_map(|c| find(c, id))
        }
        let p = find(&styled, "p").expect("styled <p>");

        // From the linked sheet.
        assert_eq!(
            p.value("background-color"),
            Some(&Value::Color(Color::rgb(9, 9, 9)))
        );
        // The <style> block comes after the <link>, so it wins for `color`.
        assert_eq!(p.value("color"), Some(&Value::Color(Color::rgb(1, 2, 3))));
    }

    #[test]
    fn runs_external_and_inline_scripts_in_document_order() {
        let document = load(&site(), "demo:///index.html");
        let path = dom_api::get_element_by_id(&document.dom, "p").unwrap();
        let element = dom_api::node_at(&document.dom, &path)
            .unwrap()
            .as_element()
            .unwrap();
        assert_eq!(element.get_attr("data-external"), Some("ran"));
        assert_eq!(element.get_attr("data-inline"), Some("ran"));
        assert!(
            document.diagnostics.is_empty(),
            "{:?}",
            document.diagnostics
        );
    }

    #[test]
    fn scripts_share_one_runtime_across_files() {
        let mut loader = MemoryLoader::new();
        loader.insert(
            "demo:///index.html",
            r#"<p id="t">x</p><script src="a.js"></script><script>document.getElementById("t").textContent = greeting;</script>"#,
        );
        loader.insert("demo:///a.js", r#"let greeting = "from external";"#);

        let document = load(&loader, "demo:///index.html");
        let path = dom_api::get_element_by_id(&document.dom, "t").unwrap();
        assert_eq!(
            dom_api::text_content(dom_api::node_at(&document.dom, &path).unwrap()),
            "from external"
        );
    }

    #[test]
    fn relative_urls_resolve_against_the_document() {
        let mut loader = MemoryLoader::new();
        loader.insert(
            "demo:///docs/guide/index.html",
            r#"<link rel="stylesheet" href="../shared/style.css"><p>x</p>"#,
        );
        loader.insert(
            "demo:///docs/shared/style.css",
            "p { color: rgb(4, 5, 6); }",
        );

        let document = load(&loader, "demo:///docs/guide/index.html");
        assert!(
            document.diagnostics.is_empty(),
            "{:?}",
            document.diagnostics
        );
        let styled = document.style_tree(800.0, &PointerState::default());
        fn first_p<'a>(node: &'a StyledNode<'a>) -> Option<&'a StyledNode<'a>> {
            if node.node.as_element().map(|e| e.tag_name.as_str()) == Some("p") {
                return Some(node);
            }
            node.children.iter().find_map(first_p)
        }
        assert_eq!(
            first_p(&styled).unwrap().value("color"),
            Some(&Value::Color(Color::rgb(4, 5, 6)))
        );
    }

    #[test]
    fn base_href_overrides_the_document_url() {
        let mut loader = MemoryLoader::new();
        loader.insert(
            "demo:///page.html",
            r#"<head><base href="/assets/"><link rel="stylesheet" href="theme.css"></head><p>x</p>"#,
        );
        loader.insert("demo:///assets/theme.css", "p { color: rgb(2, 2, 2); }");

        let document = load(&loader, "demo:///page.html");
        assert_eq!(document.base_url.to_string(), "demo:///assets/");
        assert!(
            document.diagnostics.is_empty(),
            "{:?}",
            document.diagnostics
        );
        assert_eq!(
            document.resolve("sub/x.png").unwrap().to_string(),
            "demo:///assets/sub/x.png"
        );
    }

    #[test]
    fn missing_subresources_are_reported_but_do_not_fail_the_page() {
        let mut loader = MemoryLoader::new();
        loader.insert(
            "demo:///index.html",
            r#"<link rel="stylesheet" href="gone.css"><script src="gone.js"></script><img src="gone.png"><p>still here</p>"#,
        );

        let document = load(&loader, "demo:///index.html");
        assert_eq!(document.diagnostics.len(), 3, "{:?}", document.diagnostics);
        assert!(document
            .diagnostics
            .iter()
            .all(|d| d.message.contains("not found")));
        // The page itself still renders.
        let canvas = document.render(200, 100, 0.0, &PointerState::default());
        assert_eq!(canvas.width, 200);
    }

    #[test]
    fn document_loading_fails_only_when_the_page_itself_is_missing() {
        let error = Document::load(
            &Url::parse("demo:///nope.html").unwrap(),
            &MemoryLoader::new(),
        );
        assert!(matches!(error, Err(LoadError::NotFound(_))));
    }

    #[test]
    fn images_are_fetched_and_decoded() {
        const PPM: &[u8] = b"P6\n2 2\n255\n\xff\x00\x00\x00\xff\x00\x00\x00\xff\xff\xff\xff";
        let mut loader = MemoryLoader::new();
        loader.insert(
            "demo:///a/index.html",
            r#"<img src="../pics/dot.ppm" alt="dot">"#,
        );
        loader.insert("demo:///pics/dot.ppm", PPM.to_vec());

        let document = load(&loader, "demo:///a/index.html");
        assert!(
            document.diagnostics.is_empty(),
            "{:?}",
            document.diagnostics
        );
        let url = document.resolve("../pics/dot.ppm").unwrap();
        let image = document.images.get(&url).expect("decoded image");
        assert_eq!((image.width, image.height), (2, 2));
    }

    #[test]
    fn scripts_that_add_images_get_them_loaded_on_refresh() {
        const PPM: &[u8] = b"P6\n1 1\n255\n\x10\x20\x30";
        let mut loader = MemoryLoader::new();
        loader.insert("demo:///index.html", r#"<div id="host"></div>"#);
        loader.insert("demo:///late.ppm", PPM.to_vec());

        let mut document = load(&loader, "demo:///index.html");
        assert!(document.images.is_empty());

        document.runtime.run_script(
            &mut document.dom,
            r#"const img = document.createElement("img");
               img.setAttribute("src", "late.ppm");
               document.getElementById("host").appendChild(img);"#,
        );
        document.refresh_images(&loader);

        let url = document.resolve("late.ppm").unwrap();
        assert!(
            document.images.get(&url).is_some(),
            "image added by script should load"
        );
    }

    #[test]
    fn title_and_link_resolution() {
        let mut loader = MemoryLoader::new();
        loader.insert(
            "demo:///dir/page.html",
            r#"<title>  Page  </title><p><a href="../other.html"><span>go</span></a></p>"#,
        );
        let document = load(&loader, "demo:///dir/page.html");
        assert_eq!(document.title().as_deref(), Some("Page"));

        // A click lands on the <span>; the link is found by walking up.
        let span = dom_api::query_selector(&document.dom, &[], "span").unwrap();
        assert_eq!(
            document.link_at(&span).map(|u| u.to_string()),
            Some("demo:///other.html".to_string())
        );

        let paragraph = dom_api::query_selector(&document.dom, &[], "p").unwrap();
        assert!(document.link_at(&paragraph).is_none());
    }

    #[test]
    fn external_script_src_wins_over_inline_content() {
        let mut loader = MemoryLoader::new();
        loader.insert(
            "demo:///index.html",
            r#"<p id="t">x</p><script src="a.js">document.getElementById("t").textContent = "inline";</script>"#,
        );
        loader.insert(
            "demo:///a.js",
            r#"document.getElementById("t").textContent = "external";"#,
        );

        let document = load(&loader, "demo:///index.html");
        let path = dom_api::get_element_by_id(&document.dom, "t").unwrap();
        assert_eq!(
            dom_api::text_content(dom_api::node_at(&document.dom, &path).unwrap()),
            "external"
        );
    }

    // ── Interaction: focus, keyboard, form controls ───────────────────────────

    /// Build a document from a bare HTML string, with a quiet runtime.
    fn page(html: &str) -> Document {
        let mut loader = MemoryLoader::new();
        loader.insert("demo:///page.html", html);
        let mut document =
            Document::load(&Url::parse("demo:///page.html").unwrap(), &loader).expect("page loads");
        document.runtime.quiet = true;
        document
    }

    fn path_of(document: &Document, selector: &str) -> NodePath {
        dom_api::query_selector(&document.dom, &[], selector)
            .unwrap_or_else(|| panic!("nothing matched {selector:?}"))
    }

    fn element_of<'a>(document: &'a Document, selector: &str) -> &'a ElementData {
        let path = path_of(document, selector);
        dom_api::node_at(&document.dom, &path)
            .and_then(|n| n.as_element())
            .expect("element")
    }

    fn value_of(document: &Document, selector: &str) -> String {
        element_of(document, selector).control_value()
    }

    /// Text content of the first element matching `selector`.
    fn text_of(document: &Document, selector: &str) -> String {
        let path = path_of(document, selector);
        dom_api::text_content(dom_api::node_at(&document.dom, &path).expect("node"))
    }

    fn logs(document: &Document) -> String {
        document.runtime.console.join("\n")
    }

    /// The tag/id of the focused element, for readable assertions.
    fn focused_label(document: &Document) -> Option<String> {
        let path = document.focused_path()?;
        let element = dom_api::node_at(&document.dom, &path)?.as_element()?;
        Some(
            element
                .get_attr("id")
                .map(str::to_string)
                .unwrap_or_else(|| element.tag_name.clone()),
        )
    }

    // ── Focus ─────────────────────────────────────────────────────────────

    #[test]
    fn clicking_a_control_focuses_it() {
        let mut document = page(r#"<input id="a"><input id="b">"#);
        assert_eq!(document.active_element(), None);

        let b = path_of(&document, "#b");
        document.focus_from_click(&b);
        assert_eq!(focused_label(&document).as_deref(), Some("b"));
    }

    #[test]
    fn clicking_inside_a_control_focuses_the_control_itself() {
        // The hit test can land on a text node inside a button.
        let mut document = page(r#"<button id="go"><span id="label">go</span></button>"#);
        let label = path_of(&document, "#label");
        document.focus_from_click(&label);
        assert_eq!(focused_label(&document).as_deref(), Some("go"));
    }

    #[test]
    fn clicking_nothing_focusable_clears_focus() {
        let mut document = page(r#"<input id="a"><p id="text">plain</p>"#);
        let a = path_of(&document, "#a");
        document.focus_path(&a);
        assert!(document.active_element().is_some());

        let text = path_of(&document, "#text");
        document.focus_from_click(&text);
        assert_eq!(document.active_element(), None);
    }

    #[test]
    fn tab_and_shift_tab_walk_the_focus_order() {
        let mut document = page(
            r#"<input id="one"><button id="two">b</button><input id="three" disabled><a id="four" href="x">l</a>"#,
        );
        document.press_key(&KeyEvent::new(Key::Tab));
        assert_eq!(focused_label(&document).as_deref(), Some("one"));
        document.press_key(&KeyEvent::new(Key::Tab));
        assert_eq!(focused_label(&document).as_deref(), Some("two"));
        // The disabled input is skipped.
        document.press_key(&KeyEvent::new(Key::Tab));
        assert_eq!(focused_label(&document).as_deref(), Some("four"));
        // And it wraps.
        document.press_key(&KeyEvent::new(Key::Tab));
        assert_eq!(focused_label(&document).as_deref(), Some("one"));

        let shift_tab = KeyEvent::with_modifiers(Key::Tab, Modifiers::shift());
        document.press_key(&shift_tab);
        assert_eq!(focused_label(&document).as_deref(), Some("four"));
        document.press_key(&shift_tab);
        assert_eq!(focused_label(&document).as_deref(), Some("two"));
    }

    #[test]
    fn focus_fires_blur_focusout_focus_focusin_in_order() {
        let mut document = page(
            r#"<input id="a"><input id="b">
           <script>
             for (const id of ["a", "b"]) {
                 const el = document.getElementById(id);
                 for (const type of ["focus", "blur", "focusin", "focusout"]) {
                     el.addEventListener(type, function (e) {
                         console.log(id + ":" + e.type);
                     });
                 }
             }
           </script>"#,
        );
        let a = path_of(&document, "#a");
        let b = path_of(&document, "#b");

        document.focus_path(&a);
        assert_eq!(logs(&document), "a:focus\na:focusin");

        document.runtime.console.clear();
        document.focus_path(&b);
        assert_eq!(logs(&document), "a:blur\na:focusout\nb:focus\nb:focusin");
    }

    #[test]
    fn focus_and_blur_do_not_bubble_but_focusin_and_focusout_do() {
        let mut document = page(
            r#"<div id="wrap"><input id="a"></div>
           <script>
             const wrap = document.getElementById("wrap");
             wrap.addEventListener("focus", function () { console.log("wrap:focus"); });
             wrap.addEventListener("focusin", function () { console.log("wrap:focusin"); });
             wrap.addEventListener("focusout", function () { console.log("wrap:focusout"); });
           </script>"#,
        );
        let a = path_of(&document, "#a");
        document.focus_path(&a);
        document.blur_focused();
        // `focus` never reached the ancestor; `focusin`/`focusout` did.
        assert_eq!(logs(&document), "wrap:focusin\nwrap:focusout");
    }

    #[test]
    fn document_active_element_follows_focus() {
        let mut document = page(
            r#"<input id="a"><button id="probe">p</button>
           <script>
             document.getElementById("probe").addEventListener("click", function () {
                 console.log(document.activeElement.id);
             });
           </script>"#,
        );
        let a = path_of(&document, "#a");
        document.focus_path(&a);

        let probe = path_of(&document, "#probe");
        document
            .runtime
            .dispatch_event(&mut document.dom, &probe, "click");
        assert_eq!(logs(&document), "a");
    }

    #[test]
    fn scripts_can_move_focus_with_focus_and_blur() {
        let mut document = page(
            r#"<input id="a"><input id="b">
           <script>
             document.getElementById("b").focus();
           </script>"#,
        );
        // The pending focus request is applied when the document drains it.
        document.apply_pending_actions();
        assert_eq!(focused_label(&document).as_deref(), Some("b"));

        document
            .runtime
            .run_script(&mut document.dom, r#"document.getElementById("b").blur();"#);
        document.apply_pending_actions();
        assert_eq!(document.active_element(), None);
    }

    #[test]
    fn focus_survives_dom_mutation_before_the_focused_element() {
        let mut document = page(r#"<div id="host"><input id="target"></div>"#);
        let target = path_of(&document, "#target");
        document.focus_path(&target);
        let before = document.active_element();

        // Insert a sibling *before* the focused input: its path shifts.
        document.runtime.run_script(
            &mut document.dom,
            r#"const host = document.getElementById("host");
           const first = document.createElement("input");
           host.appendChild(first);"#,
        );
        assert_eq!(document.active_element(), before, "identity is stable");
        assert_eq!(focused_label(&document).as_deref(), Some("target"));
    }

    #[test]
    fn focus_pseudo_class_styles_the_focused_element() {
        let mut document = page(
            r#"<style>input { background-color: rgb(1, 1, 1); }
                  input:focus { background-color: rgb(9, 9, 9); }
                  div:focus-within { color: rgb(5, 5, 5); }</style>
           <div id="wrap"><input id="a"></div>"#,
        );
        let styled = document.style_tree(800.0, &PointerState::default());
        fn find<'a>(node: &'a StyledNode<'a>, tag: &str) -> Option<&'a StyledNode<'a>> {
            if node.node.as_element().map(|e| e.tag_name.as_str()) == Some(tag) {
                return Some(node);
            }
            node.children.iter().find_map(|c| find(c, tag))
        }
        assert_eq!(
            find(&styled, "input").unwrap().value("background-color"),
            Some(&Value::Color(Color::rgb(1, 1, 1)))
        );
        drop(styled);

        let a = path_of(&document, "#a");
        document.focus_path(&a);
        let styled = document.style_tree(800.0, &PointerState::default());
        assert_eq!(
            find(&styled, "input").unwrap().value("background-color"),
            Some(&Value::Color(Color::rgb(9, 9, 9))),
            ":focus should win once the element is focused"
        );
        assert_eq!(
            find(&styled, "div").unwrap().value("color"),
            Some(&Value::Color(Color::rgb(5, 5, 5))),
            ":focus-within should match the ancestor"
        );
    }

    // ── Text input ────────────────────────────────────────────────────────

    #[test]
    fn typing_into_a_focused_input_updates_its_value() {
        let mut document = page(r#"<input id="a">"#);
        let a = path_of(&document, "#a");
        document.focus_path(&a);

        document.type_text("hi");
        assert_eq!(value_of(&document, "#a"), "hi");
        assert_eq!(element_of(&document, "#a").caret(), 2);
        // The attribute is untouched: only the live value changed.
        assert_eq!(element_of(&document, "#a").get_attr("value"), None);
    }

    #[test]
    fn typing_goes_nowhere_without_focus() {
        let mut document = page(r#"<input id="a">"#);
        document.type_text("hi");
        assert_eq!(value_of(&document, "#a"), "");
    }

    #[test]
    fn editing_keys_move_the_caret_and_delete_text() {
        let mut document = page(r#"<input id="a" value="abcd">"#);
        let a = path_of(&document, "#a");
        document.focus_path(&a);

        document.press_key(&KeyEvent::new(Key::End));
        assert_eq!(element_of(&document, "#a").caret(), 4);
        document.press_key(&KeyEvent::new(Key::Backspace));
        assert_eq!(value_of(&document, "#a"), "abc");

        document.press_key(&KeyEvent::new(Key::Home));
        assert_eq!(element_of(&document, "#a").caret(), 0);
        document.press_key(&KeyEvent::new(Key::Delete));
        assert_eq!(value_of(&document, "#a"), "bc");

        document.press_key(&KeyEvent::new(Key::ArrowRight));
        document.type_text("X");
        assert_eq!(value_of(&document, "#a"), "bXc");
    }

    #[test]
    fn maxlength_readonly_and_disabled_limit_editing() {
        let mut document = page(
            r#"<input id="short" maxlength="3"><input id="ro" readonly value="fixed"><input id="off" disabled value="off">"#,
        );
        let short = path_of(&document, "#short");
        document.focus_path(&short);
        document.type_text("abcdef");
        assert_eq!(value_of(&document, "#short"), "abc");

        let readonly = path_of(&document, "#ro");
        document.focus_path(&readonly);
        document.type_text("x");
        assert_eq!(value_of(&document, "#ro"), "fixed");

        // A disabled control cannot even take focus.
        let disabled = path_of(&document, "#off");
        assert!(!document.focus_path(&disabled));
        document.type_text("x");
        assert_eq!(value_of(&document, "#off"), "off");
    }

    #[test]
    fn placeholder_shows_only_while_the_value_is_empty() {
        let mut document = page(r#"<input id="a" placeholder="Search">"#);
        assert!(element_of(&document, "#a").placeholder_shown());

        let a = path_of(&document, "#a");
        document.focus_path(&a);
        document.type_text("x");
        assert!(!element_of(&document, "#a").placeholder_shown());

        document.press_key(&KeyEvent::new(Key::Backspace));
        assert!(element_of(&document, "#a").placeholder_shown());
    }

    #[test]
    fn typing_fires_keydown_input_and_keyup_in_order() {
        let mut document = page(
            r#"<input id="a">
           <script>
             const a = document.getElementById("a");
             a.addEventListener("keydown", function (e) { console.log("keydown:" + e.key); });
             a.addEventListener("input", function () { console.log("input:" + a.value); });
             a.addEventListener("keyup", function (e) { console.log("keyup:" + e.key); });
           </script>"#,
        );
        let a = path_of(&document, "#a");
        document.focus_path(&a);
        document.press_key(&KeyEvent::character('z'));

        assert_eq!(logs(&document), "keydown:z\ninput:z\nkeyup:z");
    }

    #[test]
    fn keyboard_events_expose_key_code_and_modifiers() {
        let mut document = page(
            r#"<input id="a">
           <script>
             document.getElementById("a").addEventListener("keydown", function (e) {
                 console.log(e.key + "|" + e.code + "|" + e.shiftKey + "|" + e.ctrlKey + "|" + e.altKey + "|" + e.target.id);
             });
           </script>"#,
        );
        let a = path_of(&document, "#a");
        document.focus_path(&a);
        document.key_down(&KeyEvent::with_modifiers(
            Key::Character('q'),
            Modifiers {
                shift: true,
                ctrl: true,
                alt: false,
            },
        ));
        assert_eq!(logs(&document), "q|KeyQ|true|true|false|a");

        document.runtime.console.clear();
        document.key_down(&KeyEvent::new(Key::ArrowLeft));
        assert_eq!(logs(&document), "ArrowLeft|ArrowLeft|false|false|false|a");
    }

    #[test]
    fn prevent_default_on_keydown_suppresses_the_edit() {
        let mut document = page(
            r#"<input id="a">
           <script>
             document.getElementById("a").addEventListener("keydown", function (e) {
                 if (e.key === "x") { e.preventDefault(); }
             });
           </script>"#,
        );
        let a = path_of(&document, "#a");
        document.focus_path(&a);

        document.press_key(&KeyEvent::character('x'));
        assert_eq!(value_of(&document, "#a"), "", "cancelled key must not type");

        document.press_key(&KeyEvent::character('y'));
        assert_eq!(value_of(&document, "#a"), "y");
    }

    #[test]
    fn scripts_read_and_write_the_live_value() {
        let mut document = page(r#"<input id="a" value="start">"#);
        document.runtime.run_script(
            &mut document.dom,
            r#"const a = document.getElementById("a");
           console.log(a.value);
           a.value = "set by script";"#,
        );
        assert_eq!(logs(&document), "start");
        assert_eq!(value_of(&document, "#a"), "set by script");
        // Setting the attribute changes the default, not the live value.
        document.runtime.run_script(
            &mut document.dom,
            r#"document.getElementById("a").setAttribute("value", "new default");"#,
        );
        assert_eq!(value_of(&document, "#a"), "set by script");
        assert_eq!(
            element_of(&document, "#a").get_attr("value"),
            Some("new default")
        );
    }

    // ── Textarea ──────────────────────────────────────────────────────────

    #[test]
    fn textarea_takes_its_initial_value_from_its_content() {
        let document = page("<textarea id=\"t\">line one\nline two</textarea>");
        assert_eq!(value_of(&document, "#t"), "line one\nline two");
    }

    #[test]
    fn enter_inserts_a_newline_in_a_textarea() {
        let mut document = page(r#"<textarea id="t"></textarea>"#);
        let t = path_of(&document, "#t");
        document.focus_path(&t);

        document.type_text("ab");
        document.press_key(&KeyEvent::new(Key::Enter));
        document.type_text("cd");
        assert_eq!(value_of(&document, "#t"), "ab\ncd");

        document.press_key(&KeyEvent::new(Key::ArrowUp));
        assert_eq!(element_of(&document, "#t").caret(), 2);
    }

    #[test]
    fn enter_does_not_insert_a_newline_in_a_text_input() {
        let mut document = page(r#"<input id="a">"#);
        let a = path_of(&document, "#a");
        document.focus_path(&a);
        document.type_text("ab");
        document.press_key(&KeyEvent::new(Key::Enter));
        assert_eq!(value_of(&document, "#a"), "ab");
    }

    // ── Checkbox and radio ────────────────────────────────────────────────

    #[test]
    fn clicking_a_checkbox_toggles_it_and_fires_change() {
        let mut document = page(
            r#"<input type="checkbox" id="c">
           <script>
             const c = document.getElementById("c");
             c.addEventListener("change", function () { console.log("change:" + c.checked); });
             c.addEventListener("input", function () { console.log("input"); });
           </script>"#,
        );
        let c = path_of(&document, "#c");
        assert!(!element_of(&document, "#c").is_checked());

        document.activate(&c);
        assert!(element_of(&document, "#c").is_checked());
        assert_eq!(logs(&document), "input\nchange:true");

        document.activate(&c);
        assert!(!element_of(&document, "#c").is_checked());
    }

    #[test]
    fn space_toggles_the_focused_checkbox() {
        let mut document = page(r#"<input type="checkbox" id="c">"#);
        let c = path_of(&document, "#c");
        document.focus_path(&c);
        document.press_key(&KeyEvent::character(' '));
        assert!(element_of(&document, "#c").is_checked());
    }

    #[test]
    fn a_checked_attribute_sets_the_initial_state() {
        let mut document = page(r#"<input type="checkbox" id="c" checked>"#);
        assert!(element_of(&document, "#c").is_checked());
        let c = path_of(&document, "#c");
        document.activate(&c);
        assert!(!element_of(&document, "#c").is_checked());
    }

    #[test]
    fn radios_in_a_group_are_mutually_exclusive() {
        let mut document = page(
            r#"<form>
             <input type="radio" name="pick" id="a" value="a" checked>
             <input type="radio" name="pick" id="b" value="b">
             <input type="radio" name="other" id="c" value="c" checked>
           </form>"#,
        );
        let b = path_of(&document, "#b");
        document.activate(&b);

        assert!(!element_of(&document, "#a").is_checked());
        assert!(element_of(&document, "#b").is_checked());
        // A different name is a different group.
        assert!(element_of(&document, "#c").is_checked());

        // Re-activating a checked radio keeps it checked.
        document.activate(&b);
        assert!(element_of(&document, "#b").is_checked());
    }

    #[test]
    fn scripts_see_and_set_checkedness() {
        let mut document = page(r#"<input type="checkbox" id="c">"#);
        document.runtime.run_script(
            &mut document.dom,
            r#"const c = document.getElementById("c");
           console.log(c.checked);
           c.checked = true;"#,
        );
        assert_eq!(logs(&document), "false");
        assert!(element_of(&document, "#c").is_checked());
    }

    #[test]
    fn checked_and_disabled_pseudo_classes_match_live_state() {
        let mut document = page(
            // `:checked` is last so it wins over `:enabled`, which has the same
            // specificity and also matches a checked, enabled box.
            r#"<style>input:disabled { color: rgb(3, 3, 3); }
                  input:enabled { color: rgb(4, 4, 4); }
                  input:checked { color: rgb(2, 2, 2); }</style>
           <input type="checkbox" id="c"><input id="off" disabled>"#,
        );
        fn color_of<'a>(node: &'a StyledNode<'a>, id: &str) -> Option<&'a Value> {
            if node.node.as_element().and_then(|e| e.get_attr("id")) == Some(id) {
                return node.value("color");
            }
            node.children.iter().find_map(|c| color_of(c, id))
        }

        let styled = document.style_tree(800.0, &PointerState::default());
        assert_eq!(
            color_of(&styled, "c"),
            Some(&Value::Color(Color::rgb(4, 4, 4)))
        );
        assert_eq!(
            color_of(&styled, "off"),
            Some(&Value::Color(Color::rgb(3, 3, 3)))
        );
        drop(styled);

        let c = path_of(&document, "#c");
        document.activate(&c);
        let styled = document.style_tree(800.0, &PointerState::default());
        assert_eq!(
            color_of(&styled, "c"),
            Some(&Value::Color(Color::rgb(2, 2, 2))),
            ":checked should match after the toggle"
        );
    }

    // ── Forms ─────────────────────────────────────────────────────────────

    #[test]
    fn submitting_fires_a_submit_event_and_prepares_a_get_navigation() {
        let mut document = page(
            r#"<form id="f" action="/search" method="get">
             <input name="q" value="browser engine">
             <input name="off" value="x" disabled>
             <input type="checkbox" name="box" value="1">
           </form>
           <script>
             document.getElementById("f").addEventListener("submit", function () {
                 console.log("submit");
             });
           </script>"#,
        );
        let form = path_of(&document, "#f");
        let action = document.submit_form(&form);

        assert_eq!(logs(&document), "submit");
        match action {
            PageAction::Submit(submission) => {
                // Disabled and unchecked controls are not submitted; the space is encoded.
                assert_eq!(
                    submission.url.to_string(),
                    "demo:///search?q=browser+engine"
                );
            }
            other => panic!("expected a submission, got {other:?}"),
        }
    }

    #[test]
    fn prevent_default_on_submit_cancels_the_navigation() {
        let mut document = page(
            r#"<form id="f" action="/search"><input name="q" value="x"></form>
           <script>
             document.getElementById("f").addEventListener("submit", function (e) {
                 e.preventDefault();
                 console.log("cancelled");
             });
           </script>"#,
        );
        let form = path_of(&document, "#f");
        assert_eq!(document.submit_form(&form), PageAction::None);
        assert_eq!(logs(&document), "cancelled");
    }

    #[test]
    fn enter_in_a_text_field_submits_its_form() {
        let mut document =
            page(r#"<form id="f" action="/go"><input id="q" name="q" value="hi"></form>"#);
        let q = path_of(&document, "#q");
        document.focus_path(&q);
        match document.key_down(&KeyEvent::new(Key::Enter)) {
            PageAction::Submit(submission) => {
                assert_eq!(submission.url.to_string(), "demo:///go?q=hi");
            }
            other => panic!("expected implicit submission, got {other:?}"),
        }
    }

    #[test]
    fn enter_does_not_submit_from_a_textarea() {
        let mut document = page(r#"<form id="f" action="/go"><textarea id="t"></textarea></form>"#);
        let t = path_of(&document, "#t");
        document.focus_path(&t);
        assert_eq!(
            document.key_down(&KeyEvent::new(Key::Enter)),
            PageAction::None
        );
        assert_eq!(value_of(&document, "#t"), "\n");
    }

    #[test]
    fn clicking_a_submit_button_submits_its_form() {
        let mut document = page(
            r#"<form id="f" action="/go"><input name="q" value="v"><button id="go" type="submit">Go</button></form>"#,
        );
        let go = path_of(&document, "#go");
        match document.activate(&go) {
            PageAction::Submit(submission) => {
                assert_eq!(submission.url.to_string(), "demo:///go?q=v")
            }
            other => panic!("expected a submission, got {other:?}"),
        }
    }

    #[test]
    fn a_reset_button_restores_attribute_defaults() {
        let mut document = page(
            r#"<form id="f">
             <input id="q" name="q" value="default">
             <input type="checkbox" id="c" name="c" checked>
             <button id="clear" type="reset">Reset</button>
           </form>"#,
        );
        let q = path_of(&document, "#q");
        document.focus_path(&q);
        document.press_key(&KeyEvent::new(Key::End));
        document.type_text("!");
        let c = path_of(&document, "#c");
        document.activate(&c);
        assert_eq!(value_of(&document, "#q"), "default!");
        assert!(!element_of(&document, "#c").is_checked());

        let clear = path_of(&document, "#clear");
        document.activate(&clear);
        assert_eq!(value_of(&document, "#q"), "default");
        assert!(element_of(&document, "#c").is_checked());
    }

    #[test]
    fn scripts_reach_the_form_through_elements_and_form() {
        let mut document = page(
            r#"<form id="f"><input id="q" name="q" value="v"><button>b</button></form>
           <script>
             const form = document.getElementById("f");
             console.log(form.elements.length);
             console.log(document.getElementById("q").form.id);
           </script>"#,
        );
        let _ = &mut document;
        assert_eq!(logs(&document), "2\nf");
    }

    #[test]
    fn form_submit_from_script_skips_the_submit_event() {
        let mut document = page(
            r#"<form id="f" action="/go"><input name="q" value="v"></form>
           <script>
             document.getElementById("f").addEventListener("submit", function () {
                 console.log("listener ran");
             });
             document.getElementById("f").submit();
           </script>"#,
        );
        let submission = document.apply_pending_actions().expect("script submission");
        assert_eq!(submission.url.to_string(), "demo:///go?q=v");
        assert_eq!(logs(&document), "", "form.submit() fires no submit event");
    }

    // ── Rendering ─────────────────────────────────────────────────────────

    #[test]
    fn typing_changes_the_painted_page() {
        let mut document = page(r#"<input id="a" style="width: 200px">"#);
        let before = document
            .render(300, 80, 0.0, &PointerState::default())
            .to_ppm();

        let a = path_of(&document, "#a");
        document.focus_path(&a);
        document.type_text("typed");

        let after = document
            .render(300, 80, 0.0, &PointerState::default())
            .to_ppm();
        assert_eq!(before.len(), after.len());
        assert_ne!(before, after, "typed text must reach the canvas");
    }

    #[test]
    fn focusing_a_control_changes_the_painted_page() {
        let mut document = page(r#"<input id="a">"#);
        let before = document
            .render(300, 80, 0.0, &PointerState::default())
            .to_ppm();

        let a = path_of(&document, "#a");
        document.focus_path(&a);
        let after = document
            .render(300, 80, 0.0, &PointerState::default())
            .to_ppm();
        assert_ne!(before, after, "the focus ring and caret must be visible");
    }

    #[test]
    fn toggling_a_checkbox_changes_the_painted_page() {
        let mut document = page(r#"<input type="checkbox" id="c">"#);
        let before = document
            .render(120, 60, 0.0, &PointerState::default())
            .to_ppm();

        let c = path_of(&document, "#c");
        document.activate(&c);
        let after = document
            .render(120, 60, 0.0, &PointerState::default())
            .to_ppm();
        assert_ne!(before, after, "the check mark must be visible");
    }

    #[test]
    fn clicking_into_text_places_the_caret_near_the_click() {
        let mut document = page(r#"<input id="a" value="hello world" style="width: 300px">"#);
        let a = path_of(&document, "#a");
        document.focus_path(&a);
        document.press_key(&KeyEvent::new(Key::End));
        assert_eq!(element_of(&document, "#a").caret(), 11);

        // Click near the left edge of the field's content.
        let layout_x = {
            let styled = document.style_tree(800.0, &PointerState::default());
            let layout = document.layout(&styled, 800.0);
            fn find(layout: &LayoutBox, dom: &Node, target: &[usize]) -> Option<f32> {
                if let Some(node) = layout.styled_node() {
                    if dom_api::path_of(dom, node.node).as_deref() == Some(target) {
                        return Some(layout.dimensions.content.x);
                    }
                }
                layout.children.iter().find_map(|c| find(c, dom, target))
            }
            find(&layout, &document.dom, &a).expect("input box")
        };

        document.place_caret_from_click(&a, layout_x + 1.0, 0.0, 800.0);
        assert!(
            element_of(&document, "#a").caret() <= 1,
            "clicking at the start should put the caret near it, got {}",
            element_of(&document, "#a").caret()
        );
    }

    // ── Event loop: timers, intervals and animation frames ────────────────────

    /// Run the loop at `now`, in event-loop milliseconds.
    fn tick_at(document: &mut Document, now_ms: f64) -> LoopReport {
        document.run_event_loop(now_ms)
    }

    // ── setTimeout ────────────────────────────────────────────────────────

    #[test]
    fn a_timeout_fires_only_after_its_deadline() {
        let mut document = page(
            r#"<p id="status">waiting</p>
           <script>
             setTimeout(function () {
                 document.getElementById("status").textContent = "done";
             }, 100);
           </script>"#,
        );
        assert_eq!(text_of(&document, "#status"), "waiting");

        tick_at(&mut document, 99.0);
        assert_eq!(
            text_of(&document, "#status"),
            "waiting",
            "99ms is too early"
        );

        tick_at(&mut document, 100.0);
        assert_eq!(text_of(&document, "#status"), "done");

        // One-shot: nothing more happens later.
        let report = tick_at(&mut document, 5_000.0);
        assert_eq!(report.timers_run, 0);
    }

    #[test]
    fn timeouts_run_in_deadline_then_registration_order() {
        let mut document = page(
            r#"<script>
             setTimeout(function () { console.log("a"); }, 100);
             setTimeout(function () { console.log("b"); }, 50);
             setTimeout(function () { console.log("c"); }, 100);
           </script>"#,
        );
        tick_at(&mut document, 100.0);
        assert_eq!(logs(&document), "b\na\nc");
    }

    #[test]
    fn a_missing_or_negative_delay_runs_on_the_next_turn() {
        let mut document = page(
            r#"<script>
             setTimeout(function () { console.log("negative"); }, -100);
             setTimeout(function () { console.log("zero"); }, 0);
           </script>"#,
        );
        // Nothing ran synchronously.
        assert_eq!(logs(&document), "");
        tick_at(&mut document, 0.0);
        assert_eq!(logs(&document), "negative\nzero");
    }

    #[test]
    fn clear_timeout_cancels_a_pending_callback() {
        let mut document = page(
            r#"<script>
             const id = setTimeout(function () { console.log("never"); }, 10);
             setTimeout(function () { console.log("kept"); }, 10);
             clearTimeout(id);
           </script>"#,
        );
        tick_at(&mut document, 10.0);
        assert_eq!(logs(&document), "kept");
    }

    #[test]
    fn a_callback_can_clear_another_timer() {
        let mut document = page(
            r#"<script>
             let second = 0;
             setTimeout(function () {
                 console.log("first");
                 clearTimeout(second);
             }, 10);
             second = setTimeout(function () { console.log("second"); }, 20);
           </script>"#,
        );
        tick_at(&mut document, 10.0);
        tick_at(&mut document, 20.0);
        assert_eq!(logs(&document), "first");
    }

    #[test]
    fn a_nested_zero_timeout_waits_for_the_next_turn() {
        let mut document = page(
            r#"<script>
             setTimeout(function () {
                 console.log("outer");
                 setTimeout(function () { console.log("inner"); }, 0);
             }, 0);
           </script>"#,
        );
        tick_at(&mut document, 0.0);
        assert_eq!(logs(&document), "outer", "the nested timer is a new task");

        tick_at(&mut document, 0.0);
        assert_eq!(logs(&document), "outer\ninner");
    }

    #[test]
    fn a_timer_callback_keeps_the_variables_it_closed_over() {
        let mut document = page(
            r#"<p id="status">-</p>
           <script>
             function schedule(label) {
                 let count = 0;
                 setTimeout(function () {
                     count = count + 1;
                     document.getElementById("status").textContent = label + ":" + count;
                 }, 10);
             }
             schedule("captured");
           </script>"#,
        );
        tick_at(&mut document, 10.0);
        assert_eq!(text_of(&document, "#status"), "captured:1");
    }

    #[test]
    fn a_timer_callback_can_build_dom_that_the_layout_engine_measures() {
        let mut document = page(
            r#"<ul id="list"></ul>
           <script>
             setTimeout(function () {
                 const list = document.getElementById("list");
                 for (let i = 0; i < 3; i++) {
                     const item = document.createElement("li");
                     item.textContent = "row " + i;
                     list.appendChild(item);
                 }
             }, 10);
           </script>"#,
        );
        let before = document
            .render(400, 200, 0.0, &PointerState::default())
            .to_ppm();

        tick_at(&mut document, 10.0);
        assert_eq!(
            dom_api::query_selector_all(&document.dom, &[], "li").len(),
            3
        );

        let after = document
            .render(400, 200, 0.0, &PointerState::default())
            .to_ppm();
        assert_ne!(before, after, "the new list items must be painted");
    }

    #[test]
    fn a_failing_callback_does_not_stop_the_others() {
        let mut document = page(
            r#"<script>
             setTimeout(function () { nonexistent.foo(); }, 10);
             setTimeout(function () { console.log("still running"); }, 10);
             setInterval(function () { console.log("interval alive"); }, 10);
           </script>"#,
        );
        tick_at(&mut document, 10.0);
        let output = logs(&document);
        assert!(
            output.contains("TypeError"),
            "the failure is reported: {output}"
        );
        assert!(output.contains("still running"), "{output}");

        // The page keeps working afterwards.
        tick_at(&mut document, 20.0);
        assert!(logs(&document).contains("interval alive"));
        let canvas = document.render(100, 50, 0.0, &PointerState::default());
        assert_eq!(canvas.width, 100);
    }

    // ── setInterval ───────────────────────────────────────────────────────

    #[test]
    fn an_interval_fires_repeatedly() {
        let mut document = page(
            r#"<p id="count">0</p>
           <script>
             let n = 0;
             setInterval(function () {
                 n = n + 1;
                 document.getElementById("count").textContent = "" + n;
             }, 100);
           </script>"#,
        );
        for step in 1..=3 {
            tick_at(&mut document, step as f64 * 100.0);
            assert_eq!(text_of(&document, "#count"), step.to_string());
        }
    }

    #[test]
    fn an_interval_can_stop_itself_from_its_own_callback() {
        let mut document = page(
            r#"<script>
             let n = 0;
             const id = setInterval(function () {
                 n = n + 1;
                 console.log("tick " + n);
                 if (n === 3) { clearInterval(id); }
             }, 10);
           </script>"#,
        );
        for step in 1..=6 {
            tick_at(&mut document, step as f64 * 10.0);
        }
        assert_eq!(logs(&document), "tick 1\ntick 2\ntick 3");
        assert!(!document.has_pending_tasks(), "the interval is gone");
    }

    #[test]
    fn a_zero_delay_interval_cannot_run_away() {
        let mut document = page(
            r#"<script>
             let n = 0;
             setInterval(function () { n = n + 1; console.log("" + n); }, 0);
           </script>"#,
        );
        // A single turn runs it at most once, however long the gap was.
        let report = tick_at(&mut document, 1_000.0);
        assert_eq!(report.timers_run, 1, "no catch-up storm");
        assert_eq!(logs(&document), "1");
    }

    #[test]
    fn several_intervals_keep_their_own_state() {
        let mut document = page(
            r#"<script>
             let fast = 0;
             let slow = 0;
             setInterval(function () { fast = fast + 1; }, 10);
             setInterval(function () { slow = slow + 1; }, 50);
             setTimeout(function () { console.log(fast + "/" + slow); }, 200);
           </script>"#,
        );
        // Step the loop the way a frame cadence would.
        let mut now = 0.0;
        while now < 200.0 {
            now += 10.0;
            tick_at(&mut document, now);
        }
        assert_eq!(logs(&document), "20/4");
    }

    // ── requestAnimationFrame ─────────────────────────────────────────────

    #[test]
    fn an_animation_frame_runs_once_with_the_frame_timestamp() {
        let mut document = page(
            r#"<script>
             requestAnimationFrame(function (t) { console.log("frame " + t); });
           </script>"#,
        );
        document.run_animation_frames(16.0);
        assert_eq!(logs(&document), "frame 16");

        // Single-shot: the next frame runs nothing.
        document.run_animation_frames(32.0);
        assert_eq!(logs(&document), "frame 16");
    }

    #[test]
    fn every_callback_in_a_frame_sees_the_same_timestamp() {
        let mut document = page(
            r#"<script>
             requestAnimationFrame(function (t) { console.log("a" + t); });
             requestAnimationFrame(function (t) { console.log("b" + t); });
           </script>"#,
        );
        document.run_animation_frames(50.0);
        assert_eq!(
            logs(&document),
            "a50\nb50",
            "same timestamp, registration order"
        );
    }

    #[test]
    fn a_frame_requested_inside_a_frame_runs_on_the_next_one() {
        let mut document = page(
            r#"<script>
             let frames = 0;
             function step(t) {
                 frames = frames + 1;
                 console.log("step " + frames + " at " + t);
                 if (frames < 3) { requestAnimationFrame(step); }
             }
             requestAnimationFrame(step);
           </script>"#,
        );
        document.run_animation_frames(16.0);
        assert_eq!(logs(&document), "step 1 at 16");

        document.run_animation_frames(32.0);
        document.run_animation_frames(48.0);
        assert_eq!(logs(&document), "step 1 at 16\nstep 2 at 32\nstep 3 at 48");

        // The loop stopped asking for frames.
        document.run_animation_frames(64.0);
        assert_eq!(logs(&document).lines().count(), 3);
    }

    #[test]
    fn an_animation_frame_can_be_cancelled() {
        let mut document = page(
            r#"<script>
             const id = requestAnimationFrame(function () { console.log("never"); });
             requestAnimationFrame(function () { console.log("kept"); });
             cancelAnimationFrame(id);
           </script>"#,
        );
        document.run_animation_frames(16.0);
        assert_eq!(logs(&document), "kept");
    }

    #[test]
    fn a_style_change_in_an_animation_frame_reaches_the_paint() {
        let mut document = page(
            r#"<div id="box" style="width: 20px; height: 20px; background-color: rgb(200, 0, 0)"></div>
           <script>
             let x = 0;
             function step() {
                 x = x + 20;
                 document.getElementById("box").style.marginLeft = x + "px";
                 if (x < 60) { requestAnimationFrame(step); }
             }
             requestAnimationFrame(step);
           </script>"#,
        );
        let mut frames = Vec::new();
        for frame in 0..3 {
            document.run_animation_frames(frame as f64 * 16.0);
            frames.push(
                document
                    .render(200, 60, 0.0, &PointerState::default())
                    .to_ppm(),
            );
        }
        assert_ne!(frames[0], frames[1], "the box should have moved");
        assert_ne!(frames[1], frames[2], "and moved again");
    }

    // ── performance.now ───────────────────────────────────────────────────

    #[test]
    fn performance_now_reports_event_loop_time() {
        let mut document = page(
            r#"<script>
             console.log("" + performance.now());
             setTimeout(function () { console.log("" + performance.now()); }, 250);
           </script>"#,
        );
        assert_eq!(logs(&document), "0", "time starts at page load");

        tick_at(&mut document, 250.0);
        assert_eq!(
            logs(&document),
            "0\n250",
            "deterministic under a test clock"
        );
    }

    // ── Loop bookkeeping ──────────────────────────────────────────────────

    #[test]
    fn the_document_reports_when_it_next_needs_a_turn() {
        let mut document = page(
            r#"<script>
             setTimeout(function () {}, 200);
             setTimeout(function () {}, 50);
           </script>"#,
        );
        assert_eq!(document.next_wakeup_ms(), Some(50.0));
        assert!(document.has_pending_tasks());

        tick_at(&mut document, 50.0);
        assert_eq!(document.next_wakeup_ms(), Some(200.0));

        tick_at(&mut document, 200.0);
        assert_eq!(document.next_wakeup_ms(), None, "idle page");
        assert!(!document.has_pending_tasks());
    }

    #[test]
    fn a_pending_animation_frame_asks_for_an_immediate_turn() {
        let document = page(r#"<script>requestAnimationFrame(function () {});</script>"#);
        assert_eq!(document.next_wakeup_ms(), Some(0.0));
    }

    #[test]
    fn cancelling_every_task_empties_the_queues() {
        let mut document = page(
            r#"<script>
             setInterval(function () { console.log("tick"); }, 10);
             requestAnimationFrame(function () { console.log("frame"); });
           </script>"#,
        );
        document.cancel_all_tasks();
        assert!(!document.has_pending_tasks());

        let report = tick_at(&mut document, 1_000.0);
        assert!(!report.did_work());
        assert_eq!(logs(&document), "");
    }

    #[test]
    fn an_event_listener_can_schedule_a_timer() {
        let mut document = page(
            r#"<button id="go">go</button><p id="status">idle</p>
           <script>
             document.getElementById("go").addEventListener("click", function () {
                 document.getElementById("status").textContent = "starting";
                 setTimeout(function () {
                     document.getElementById("status").textContent = "finished";
                 }, 100);
             });
           </script>"#,
        );
        let go = path_of(&document, "#go");
        document
            .runtime
            .dispatch_event(&mut document.dom, &go, "click");
        document.apply_pending_actions();
        assert_eq!(text_of(&document, "#status"), "starting");

        tick_at(&mut document, 50.0);
        assert_eq!(text_of(&document, "#status"), "starting");
        tick_at(&mut document, 100.0);
        assert_eq!(text_of(&document, "#status"), "finished");
    }

    // ── Microtasks and promises ───────────────────────────────────────────────

    // ── queueMicrotask ────────────────────────────────────────────────────

    #[test]
    fn a_microtask_runs_after_the_script_that_queued_it() {
        let document = page(
            r#"<script>
             console.log("A");
             queueMicrotask(function () { console.log("B"); });
             console.log("C");
           </script>"#,
        );
        // The checkpoint after the script has already run by the time we look.
        assert_eq!(logs(&document), "A\nC\nB");
    }

    #[test]
    fn microtasks_run_in_the_order_they_were_queued() {
        let document = page(
            r#"<script>
             queueMicrotask(function () { console.log("a"); });
             Promise.resolve().then(function () { console.log("b"); });
             queueMicrotask(function () { console.log("c"); });
           </script>"#,
        );
        assert_eq!(logs(&document), "a\nb\nc", "one shared FIFO queue");
    }

    #[test]
    fn a_microtask_queued_by_a_microtask_runs_in_the_same_checkpoint() {
        let document = page(
            r#"<script>
             queueMicrotask(function () {
                 console.log("first");
                 queueMicrotask(function () { console.log("second"); });
             });
           </script>"#,
        );
        assert_eq!(logs(&document), "first\nsecond");
    }

    #[test]
    fn a_failing_microtask_does_not_stop_the_others() {
        let document = page(
            r#"<script>
             queueMicrotask(function () { nonexistent.foo(); });
             queueMicrotask(function () { console.log("still ran"); });
           </script>"#,
        );
        let output = logs(&document);
        assert!(output.contains("TypeError"), "reported: {output}");
        assert!(output.contains("still ran"), "{output}");
    }

    #[test]
    fn a_microtask_that_requeues_itself_forever_yields_instead_of_hanging() {
        let mut document = page(
            r#"<script>
             function spin() { queueMicrotask(spin); }
             spin();
           </script>"#,
        );
        // The load checkpoint hit its budget and stopped rather than looping.
        assert!(
            logs(&document).contains("budget exhausted"),
            "expected a diagnostic, got {:?}",
            logs(&document)
        );
        assert!(document.has_pending_microtasks(), "the rest were deferred");

        // The page still works afterwards.
        document.runtime.console.clear();
        document
            .runtime
            .run_script(&mut document.dom, r#"console.log("alive");"#);
        assert!(logs(&document).contains("alive"));
    }

    // ── Task vs microtask ordering ────────────────────────────────────────

    #[test]
    fn microtasks_run_before_timers() {
        let mut document = page(
            r#"<script>
             console.log("1");
             setTimeout(function () { console.log("timeout"); }, 0);
             queueMicrotask(function () { console.log("microtask"); });
             console.log("2");
           </script>"#,
        );
        assert_eq!(
            logs(&document),
            "1\n2\nmicrotask",
            "the timer has not run yet"
        );

        tick_at(&mut document, 0.0);
        assert_eq!(logs(&document), "1\n2\nmicrotask\ntimeout");
    }

    #[test]
    fn a_microtask_from_a_timer_runs_before_the_next_timer() {
        let mut document = page(
            r#"<script>
             setTimeout(function () {
                 console.log("timer1");
                 queueMicrotask(function () { console.log("micro"); });
             }, 0);
             setTimeout(function () { console.log("timer2"); }, 0);
           </script>"#,
        );
        tick_at(&mut document, 0.0);
        assert_eq!(logs(&document), "timer1\nmicro\ntimer2");
    }

    #[test]
    fn a_promise_resolved_in_an_animation_frame_settles_before_the_paint() {
        let mut document = page(
            r#"<script>
             requestAnimationFrame(function () {
                 console.log("frame");
                 Promise.resolve().then(function () { console.log("after frame"); });
             });
           </script>"#,
        );
        document.run_animation_frames(16.0);
        assert_eq!(logs(&document), "frame\nafter frame");
    }

    // ── fetch(): the pipeline ─────────────────────────────────────────────

    /// A page wired to a network that completes only when the test says so.
    fn fetching_page(html: &str) -> (Document, Rc<ManualNetwork>) {
        let mut document = page(html);
        let network = Rc::new(ManualNetwork::new());
        document.set_network(network.clone());
        (document, network)
    }

    /// Send the queued requests, complete them all, then deliver the answers.
    ///
    /// Two turns, because a request started in one turn is never collected in
    /// the same one.
    fn run_fetches(document: &mut Document, network: &ManualNetwork) {
        document.run_event_loop(0.0);
        network.complete_all();
        document.run_event_loop(0.0);
    }

    #[test]
    fn fetch_returns_a_pending_promise_without_doing_any_work() {
        let (document, network) = fetching_page(
            r#"<script>
                 console.log("A");
                 fetch("data.json").then(function () { console.log("C"); });
                 console.log("B");
               </script>"#,
        );

        // The script and the load checkpoint have both finished, and the
        // handler has not run: nothing was fetched on the call stack.
        assert_eq!(logs(&document), "A\nB");
        assert_eq!(document.in_flight_requests(), 1);
        assert_eq!(
            network.pending_count(),
            0,
            "the request has not even reached the network yet"
        );
    }

    #[test]
    fn a_request_is_sent_on_the_next_turn_and_collected_on_the_one_after() {
        let (mut document, network) = fetching_page(
            r#"<script>fetch("data.json").then(function () { console.log("done"); });</script>"#,
        );

        let first = document.run_event_loop(0.0);
        assert_eq!(first.requests_sent, 1);
        assert_eq!(first.network_completions, 0);
        assert_eq!(network.pending_count(), 1);
        assert_eq!(
            logs(&document),
            "",
            "a fast source still cannot be same-turn"
        );

        network.complete_all();
        let second = document.run_event_loop(0.0);
        assert_eq!(second.network_completions, 1);
        assert_eq!(logs(&document), "done");
        assert_eq!(document.in_flight_requests(), 0);
    }

    #[test]
    fn a_completion_is_a_task_and_its_reactions_are_microtasks() {
        let (mut document, network) = fetching_page(
            r#"<script>
                 fetch("data.json").then(function () {
                     console.log("reaction");
                     queueMicrotask(function () { console.log("micro from reaction"); });
                 });
                 setTimeout(function () { console.log("timer"); }, 0);
               </script>"#,
        );

        document.run_event_loop(0.0);
        network.complete_all();
        // The completion runs before the timer in the same turn, and its
        // reactions drain before the timer gets a chance.
        assert_eq!(
            logs(&document),
            "timer",
            "the first turn only had the timer to run"
        );

        document.run_event_loop(0.0);
        assert_eq!(logs(&document), "timer\nreaction\nmicro from reaction");
    }

    #[test]
    fn the_resolved_value_is_a_response() {
        let (mut document, network) = fetching_page(
            r#"<script>
                 fetch("data.json").then(function (r) {
                     console.log(r.status + " " + r.statusText + " ok=" + r.ok);
                     console.log("url " + r.url);
                     console.log("redirected " + r.redirected);
                     console.log("type " + r.type);
                 });
               </script>"#,
        );
        network.respond_text("demo:///data.json", "hello");
        run_fetches(&mut document, &network);

        assert_eq!(
            logs(&document),
            "200 OK ok=true\nurl demo:///data.json\nredirected false\ntype basic"
        );
    }

    // ── URLs ──────────────────────────────────────────────────────────────

    #[test]
    fn a_relative_fetch_url_resolves_against_the_document() {
        let mut document = page(r#"<script>fetch("api/data.json");</script>"#);
        let network = Rc::new(ManualNetwork::new());
        document.set_network(network.clone());
        document.run_event_loop(0.0);

        assert_eq!(
            network.pending()[0].1,
            "demo:///api/data.json",
            "a relative reference is joined to the page URL"
        );
    }

    #[test]
    fn every_url_shape_resolves() {
        let (mut document, network) = fetching_page(
            r#"<script>
                 fetch("sibling.json");
                 fetch("/root.json");
                 fetch("../up.json");
                 fetch("q.json?a=1#frag");
                 fetch("demo:///absolute.json");
               </script>"#,
        );
        document.run_event_loop(0.0);

        let sent: Vec<String> = network.pending().into_iter().map(|(_, url)| url).collect();
        assert_eq!(
            sent,
            vec![
                "demo:///sibling.json",
                "demo:///root.json",
                "demo:///up.json",
                "demo:///q.json?a=1#frag",
                "demo:///absolute.json",
            ]
        );
    }

    #[test]
    fn an_invalid_url_rejects_rather_than_crashing() {
        let (document, _network) = fetching_page(
            r#"<script>
                 fetch("").catch(function (e) { console.log("caught: " + e); });
                 console.log("still running");
               </script>"#,
        );

        assert_eq!(
            logs(&document),
            "still running\ncaught: TypeError: invalid URL: (empty)",
            "the rejection is asynchronous, and the script kept going"
        );
        assert_eq!(document.in_flight_requests(), 0);
    }

    // ── Status semantics ──────────────────────────────────────────────────

    #[test]
    fn an_error_status_resolves_rather_than_rejecting() {
        let (mut document, network) = fetching_page(
            r#"<script>
                 fetch("missing.json")
                     .then(function (r) { console.log("resolved " + r.status + " ok=" + r.ok); })
                     .catch(function (e) { console.log("REJECTED " + e); });
               </script>"#,
        );
        network.respond_with("demo:///missing.json", 404, "text/plain", "gone");
        run_fetches(&mut document, &network);

        assert_eq!(logs(&document), "resolved 404 ok=false");
    }

    #[test]
    fn every_status_class_comes_back_as_a_response() {
        for (status, ok) in [(200, true), (204, true), (404, false), (500, false)] {
            let (mut document, network) = fetching_page(
                r#"<script>
                     fetch("x").then(function (r) { console.log(r.status + "/" + r.ok); });
                   </script>"#,
            );
            network.respond_with("demo:///x", status, "text/plain", "");
            run_fetches(&mut document, &network);
            assert_eq!(logs(&document), format!("{status}/{ok}"));
        }
    }

    #[test]
    fn a_network_failure_rejects() {
        let (mut document, network) = fetching_page(
            r#"<script>
                 fetch("down")
                     .then(function () { console.log("RESOLVED"); })
                     .catch(function (e) { console.log("caught: " + e); });
               </script>"#,
        );
        network.fail(
            "demo:///down",
            crate::net::FetchError::Io("connection refused".into()),
        );
        run_fetches(&mut document, &network);

        assert!(
            logs(&document).contains("connection refused"),
            "{}",
            logs(&document)
        );
    }

    #[test]
    fn a_redirect_is_reported_on_the_response() {
        let (mut document, network) = fetching_page(
            r#"<script>
                 fetch("start")
                     .then(function (r) { console.log(r.url + " redirected=" + r.redirected); });
               </script>"#,
        );
        let mut response = crate::net::FetchResponse::synthetic(
            Url::parse("demo:///final").unwrap(),
            200,
            Some("text/plain"),
            b"arrived".to_vec(),
        );
        response.redirected = true;
        network.respond("demo:///start", response);
        run_fetches(&mut document, &network);

        assert_eq!(logs(&document), "demo:///final redirected=true");
    }

    // ── Bodies ────────────────────────────────────────────────────────────

    #[test]
    fn text_reads_the_body_as_a_promise() {
        let (mut document, network) = fetching_page(
            r#"<script>
                 fetch("note.txt")
                     .then(function (r) {
                         console.log("bodyUsed before " + r.bodyUsed);
                         const p = r.text();
                         console.log("bodyUsed after " + r.bodyUsed);
                         return p;
                     })
                     .then(function (text) { console.log("text: " + text); });
               </script>"#,
        );
        network.respond_text("demo:///note.txt", "a plain note");
        run_fetches(&mut document, &network);

        assert_eq!(
            logs(&document),
            "bodyUsed before false\nbodyUsed after true\ntext: a plain note"
        );
    }

    #[test]
    fn json_parses_the_body_into_a_value() {
        let (mut document, network) = fetching_page(
            r#"<script>
                 fetch("data.json")
                     .then(function (r) { return r.json(); })
                     .then(function (data) {
                         console.log(data.message + " / " + data.items.length + " / " + data.items[1].name);
                     });
               </script>"#,
        );
        network.respond_json(
            "demo:///data.json",
            r#"{"message":"hi","items":[{"name":"a"},{"name":"b"}]}"#,
        );
        run_fetches(&mut document, &network);

        assert_eq!(logs(&document), "hi / 2 / b");
    }

    #[test]
    fn invalid_json_rejects_with_a_reason() {
        let (mut document, network) = fetching_page(
            r#"<script>
                 fetch("broken.json")
                     .then(function (r) { return r.json(); })
                     .then(function () { console.log("RESOLVED"); })
                     .catch(function (e) { console.log("caught: " + e); });
               </script>"#,
        );
        network.respond_with("demo:///broken.json", 200, "application/json", "{not json");
        run_fetches(&mut document, &network);

        assert!(
            logs(&document).starts_with("caught: SyntaxError"),
            "{}",
            logs(&document)
        );
    }

    #[test]
    fn a_body_may_be_read_only_once() {
        let (mut document, network) = fetching_page(
            r#"<script>
                 fetch("note.txt").then(function (r) {
                     r.text().then(function (t) { console.log("first: " + t); });
                     r.text().catch(function (e) { console.log("second: " + e); });
                     r.json().catch(function (e) { console.log("third: " + e); });
                 });
               </script>"#,
        );
        network.respond_text("demo:///note.txt", "once");
        run_fetches(&mut document, &network);

        assert_eq!(
            logs(&document),
            "first: once\nsecond: TypeError: body stream already read\n\
             third: TypeError: body stream already read"
        );
    }

    #[test]
    fn an_empty_body_reads_as_an_empty_string() {
        let (mut document, network) = fetching_page(
            r#"<script>
                 fetch("empty")
                     .then(function (r) { return r.text(); })
                     .then(function (t) { console.log("[" + t + "] length " + t.length); });
               </script>"#,
        );
        network.respond_with("demo:///empty", 204, "text/plain", "");
        run_fetches(&mut document, &network);

        assert_eq!(logs(&document), "[] length 0");
    }

    #[test]
    fn text_is_never_delivered_synchronously() {
        let (mut document, network) = fetching_page(
            r#"<script>
                 fetch("note.txt").then(function (r) {
                     r.text().then(function () { console.log("body"); });
                     console.log("after calling text()");
                 });
               </script>"#,
        );
        network.respond_text("demo:///note.txt", "in memory already");
        run_fetches(&mut document, &network);

        assert_eq!(
            logs(&document),
            "after calling text()\nbody",
            "the bytes are local, but the promise is still a promise"
        );
    }

    // ── Headers ───────────────────────────────────────────────────────────

    #[test]
    fn response_headers_are_case_insensitive() {
        let (mut document, network) = fetching_page(
            r#"<script>
                 fetch("data.json").then(function (r) {
                     console.log(r.headers.get("content-type"));
                     console.log(r.headers.get("Content-Type"));
                     console.log(r.headers.has("CONTENT-LENGTH"));
                     console.log(r.headers.get("x-absent"));
                 });
               </script>"#,
        );
        network.respond_json("demo:///data.json", "{}");
        run_fetches(&mut document, &network);

        assert_eq!(
            logs(&document),
            "application/json\napplication/json\ntrue\nnull"
        );
    }

    #[test]
    fn the_headers_constructor_takes_an_object_or_another_headers() {
        let document = page(
            r#"<script>
                 const a = new Headers({ "Content-Type": "text/plain", "X-Tag": "one" });
                 console.log(a.get("content-type"));

                 const b = new Headers(a);
                 b.append("x-tag", "two");
                 console.log(b.get("x-tag"));
                 console.log(a.get("x-tag") + " (the copy is independent)");

                 b.set("x-tag", "only");
                 console.log(b.get("x-tag"));
                 b.delete("x-tag");
                 console.log(b.has("x-tag") + " " + b.get("x-tag"));
               </script>"#,
        );

        assert_eq!(
            logs(&document),
            "text/plain\none, two\none (the copy is independent)\nonly\nfalse null"
        );
    }

    #[test]
    fn headers_reject_a_forged_newline_and_ignore_forbidden_names() {
        let document = page(
            r#"<script>
                 const h = new Headers();
                 try { h.set("X-Evil", "a\nX-Injected: yes"); }
                 catch (e) { console.log("caught: " + e); }

                 h.set("Host", "evil.example");
                 console.log("host " + h.get("host"));
               </script>"#,
        );

        assert!(
            logs(&document).contains("caught: TypeError"),
            "{}",
            logs(&document)
        );
        assert!(logs(&document).contains("host null"), "{}", logs(&document));
    }

    #[test]
    fn request_headers_reach_the_network() {
        let (mut document, network) = fetching_page(
            r#"<script>
                 fetch("api", { headers: { "X-Token": "abc", "Accept": "application/json" } });
               </script>"#,
        );
        document.run_event_loop(0.0);

        let sent = network.requests();
        assert_eq!(sent[0].headers.get("x-token").as_deref(), Some("abc"));
        assert_eq!(
            sent[0].headers.get("accept").as_deref(),
            Some("application/json")
        );
    }

    // ── Requests ──────────────────────────────────────────────────────────

    #[test]
    fn a_request_object_carries_its_url_method_and_headers() {
        let document = page(
            r#"<script>
                 const r = new Request("api/save", {
                     method: "post",
                     headers: { "Content-Type": "application/json" },
                     body: "{}"
                 });
                 console.log(r.url);
                 console.log(r.method);
                 console.log(r.headers.get("content-type"));
                 console.log("bodyUsed " + r.bodyUsed);
               </script>"#,
        );

        assert_eq!(
            logs(&document),
            "demo:///api/save\nPOST\napplication/json\nbodyUsed false"
        );
    }

    #[test]
    fn fetch_accepts_a_request_object() {
        let (mut document, network) = fetching_page(
            r#"<script>
                 const r = new Request("api/save", { method: "PUT", body: "payload" });
                 fetch(r).then(function (response) { console.log("sent, got " + response.status); });
                 console.log("bodyUsed after fetch " + r.bodyUsed);
               </script>"#,
        );
        network.respond_with("demo:///api/save", 201, "text/plain", "created");
        run_fetches(&mut document, &network);

        let sent = network.requests();
        assert_eq!(sent[0].method, crate::net::Method::Put);
        assert_eq!(sent[0].body.as_deref(), Some(&b"payload"[..]));
        assert_eq!(
            logs(&document),
            "bodyUsed after fetch true\nsent, got 201",
            "Fetch synchronously disturbs the Request input body"
        );
    }

    #[test]
    fn a_post_body_and_content_type_go_on_the_wire() {
        let (mut document, network) = fetching_page(
            r#"<script>
                 fetch("api", {
                     method: "POST",
                     headers: { "Content-Type": "application/json" },
                     body: JSON.stringify({ name: "toy", count: 2 })
                 });
               </script>"#,
        );
        document.run_event_loop(0.0);

        let sent = network.requests();
        assert_eq!(sent[0].method, crate::net::Method::Post);
        assert_eq!(
            sent[0].headers.get("content-type").as_deref(),
            Some("application/json")
        );
        assert_eq!(
            String::from_utf8_lossy(sent[0].body.as_deref().unwrap()),
            r#"{"name":"toy","count":2}"#
        );
    }

    #[test]
    fn methods_are_normalised_and_unknown_ones_reject() {
        let (mut document, network) = fetching_page(
            r#"<script>
                 fetch("a", { method: "delete" });
                 fetch("b", { method: "TRACE" })
                     .catch(function (e) { console.log("caught: " + e); });
               </script>"#,
        );
        document.run_event_loop(0.0);

        assert_eq!(network.requests()[0].method, crate::net::Method::Delete);
        assert_eq!(
            network.requests().len(),
            1,
            "TRACE never reached the network"
        );
        assert!(
            logs(&document).contains("unsupported request method: TRACE"),
            "{}",
            logs(&document)
        );
    }

    #[test]
    fn a_body_on_a_get_is_refused() {
        let (document, _network) = fetching_page(
            r#"<script>
                 fetch("a", { body: "nope" }).catch(function (e) { console.log("caught: " + e); });
               </script>"#,
        );

        assert!(
            logs(&document).contains("a GET request cannot have a body"),
            "{}",
            logs(&document)
        );
    }

    #[test]
    fn head_keeps_the_metadata_and_drops_the_body() {
        let (mut document, network) = fetching_page(
            r#"<script>
                 fetch("doc.html", { method: "HEAD" })
                     .then(function (r) {
                         console.log(r.status + " " + r.headers.get("content-type"));
                         return r.text();
                     })
                     .then(function (t) { console.log("body length " + t.length); });
               </script>"#,
        );
        network.respond_with("demo:///doc.html", 200, "text/html", "<p>a whole page</p>");
        run_fetches(&mut document, &network);

        assert_eq!(logs(&document), "200 text/html\nbody length 0");
    }

    // ── The same-origin policy ────────────────────────────────────────────

    #[test]
    fn a_cross_origin_fetch_is_blocked() {
        let (document, _network) = fetching_page(
            r#"<script>
                 fetch("http://elsewhere.example/data")
                     .catch(function (e) { console.log("caught: " + e); });
               </script>"#,
        );

        assert!(
            logs(&document).contains("blocked by the same-origin policy"),
            "{}",
            logs(&document)
        );
    }

    #[test]
    fn a_local_page_cannot_climb_out_of_its_directory() {
        let mut loader = MemoryLoader::new();
        loader.insert(
            "demo:///site/page.html",
            r#"<script>
                 fetch("api/ok.json").catch(function (e) { console.log("near: " + e); });
                 fetch("../secrets.txt").catch(function (e) { console.log("far: " + e); });
                 fetch("file:///etc/passwd").catch(function (e) { console.log("disk: " + e); });
               </script>"#,
        );
        let mut document =
            Document::load(&Url::parse("demo:///site/page.html").unwrap(), &loader).unwrap();
        document.runtime.quiet = true;
        document.set_network(Rc::new(ManualNetwork::new()));

        let text = logs(&document);
        assert!(
            !text.contains("near:"),
            "a sibling resource is allowed: {text}"
        );
        assert!(text.contains("far: TypeError"), "{text}");
        assert!(text.contains("disk: TypeError"), "{text}");
    }

    #[test]
    fn an_unsupported_scheme_rejects_with_an_explanation() {
        let (document, _network) = fetching_page(
            r#"<script>
                 fetch("https://example.com/x").catch(function (e) { console.log("tls: " + e); });
                 fetch("javascript:alert(1)").catch(function (e) { console.log("js: " + e); });
               </script>"#,
        );

        let text = logs(&document);
        assert!(text.contains("tls: TypeError"), "{text}");
        assert!(text.contains("js: TypeError"), "{text}");
    }

    #[test]
    fn unsupported_init_members_are_refused_rather_than_ignored() {
        let (document, _network) = fetching_page(
            r#"<script>
                 fetch("a", { mode: "navigate" }).catch(function (e) { console.log("mode: " + e); });
                 fetch("b", { credentials: "include" }).catch(function (e) { console.log("creds: " + e); });
                 fetch("c", { mode: "same-origin" }).catch(function (e) { console.log("UNEXPECTED " + e); });
               </script>"#,
        );

        let text = logs(&document);
        assert!(text.contains("mode: TypeError"), "{text}");
        assert!(text.contains("creds: TypeError"), "{text}");
        assert!(!text.contains("UNEXPECTED"), "{text}");
    }

    // ── Concurrency and limits ────────────────────────────────────────────

    #[test]
    fn promise_all_over_several_fetches_keeps_the_input_order() {
        let (mut document, network) = fetching_page(
            r#"<script>
                 Promise.all([fetch("a"), fetch("b"), fetch("c")])
                     .then(function (responses) {
                         const urls = responses.map(function (r) { return r.url; });
                         console.log(urls.join(" | "));
                     });
               </script>"#,
        );
        for name in ["a", "b", "c"] {
            network.respond_text(&format!("demo:///{name}"), name);
        }
        document.run_event_loop(0.0);

        // Complete them out of order: b, then c, then a.
        let ids: Vec<u64> = network.pending().into_iter().map(|(id, _)| id).collect();
        network.complete(ids[1]);
        network.complete(ids[2]);
        network.complete(ids[0]);
        document.run_event_loop(0.0);

        assert_eq!(logs(&document), "demo:///a | demo:///b | demo:///c");
    }

    #[test]
    fn two_fetches_of_one_url_are_two_requests() {
        let (mut document, network) =
            fetching_page(r#"<script>fetch("data"); fetch("data");</script>"#);
        document.run_event_loop(0.0);

        assert_eq!(
            network.requests().len(),
            2,
            "there is no cache: each call is its own request"
        );
    }

    #[test]
    fn a_runaway_loop_of_fetches_is_refused_rather_than_queued() {
        let (document, _network) = fetching_page(
            r#"<script>
                 let rejected = 0;
                 for (let i = 0; i < 50; i++) {
                     fetch("item" + i).catch(function () { rejected++; });
                 }
                 queueMicrotask(function () { console.log("rejected " + rejected); });
               </script>"#,
        );

        assert_eq!(
            document.in_flight_requests(),
            crate::net::fetch::MAX_IN_FLIGHT_FETCHES
        );
        assert_eq!(
            logs(&document),
            format!("rejected {}", 50 - crate::net::fetch::MAX_IN_FLIGHT_FETCHES)
        );
    }

    // ── AbortController ───────────────────────────────────────────────────

    #[test]
    fn aborting_a_pending_request_rejects_it() {
        let (mut document, network) = fetching_page(
            r#"<script>
                 const controller = new AbortController();
                 console.log("aborted before " + controller.signal.aborted);
                 fetch("slow", { signal: controller.signal })
                     .then(function () { console.log("RESOLVED"); })
                     .catch(function (e) { console.log("caught: " + e); });
                 controller.abort();
                 console.log("aborted after " + controller.signal.aborted);
               </script>"#,
        );
        network.respond_text("demo:///slow", "too late");

        assert_eq!(
            logs(&document),
            "aborted before false\naborted after true\ncaught: AbortError: the request was aborted"
        );
        assert_eq!(document.in_flight_requests(), 0);

        // The answer arriving afterwards changes nothing.
        run_fetches(&mut document, &network);
        assert!(!logs(&document).contains("RESOLVED"));
    }

    #[test]
    fn a_fetch_with_an_already_aborted_signal_never_starts() {
        let (document, network) = fetching_page(
            r#"<script>
                 const controller = new AbortController();
                 controller.abort();
                 fetch("x", { signal: controller.signal })
                     .catch(function (e) { console.log("caught: " + e); });
               </script>"#,
        );

        assert!(
            logs(&document).contains("AbortError"),
            "{}",
            logs(&document)
        );
        assert_eq!(document.in_flight_requests(), 0);
        assert_eq!(network.requests().len(), 0);
    }

    #[test]
    fn aborting_after_completion_does_nothing() {
        let (mut document, network) = fetching_page(
            r#"<script>
                 const controller = new AbortController();
                 fetch("x", { signal: controller.signal })
                     .then(function (r) { console.log("resolved " + r.status); controller.abort(); })
                     .catch(function (e) { console.log("caught: " + e); });
               </script>"#,
        );
        network.respond_text("demo:///x", "in time");
        run_fetches(&mut document, &network);

        assert_eq!(logs(&document), "resolved 200");
    }

    #[test]
    fn one_signal_aborts_every_request_watching_it() {
        let (document, _network) = fetching_page(
            r#"<script>
                 const controller = new AbortController();
                 const signal = controller.signal;
                 fetch("a", { signal: signal }).catch(function () { console.log("a aborted"); });
                 fetch("b", { signal: signal }).catch(function () { console.log("b aborted"); });
                 fetch("c").then(function () { console.log("c survived"); });
                 controller.abort();
               </script>"#,
        );

        assert_eq!(logs(&document), "a aborted\nb aborted");
        assert_eq!(document.in_flight_requests(), 1, "c is still going");
    }

    // ── Lifecycle ─────────────────────────────────────────────────────────

    #[test]
    fn dropping_a_document_releases_its_pending_promises() {
        let (mut document, _network) = fetching_page(r#"<script>fetch("a"); fetch("b");</script>"#);
        assert_eq!(document.in_flight_requests(), 2);

        // What navigating away does.
        document.cancel_all_tasks();
        assert_eq!(document.in_flight_requests(), 0);
        assert!(!document.has_pending_network());
    }

    #[test]
    fn a_late_completion_for_a_cancelled_request_is_discarded() {
        let (mut document, network) = fetching_page(
            r#"<script>
                 fetch("slow").then(function () { console.log("SHOULD NOT RUN"); });
               </script>"#,
        );
        network.respond_text("demo:///slow", "answer");
        document.run_event_loop(0.0);

        // The page goes away while the answer is on its way back.
        document.cancel_all_tasks();
        network.complete_all();
        let report = document.run_event_loop(0.0);

        assert_eq!(report.network_completions, 0);
        assert_eq!(logs(&document), "");
    }

    #[test]
    fn cancelling_tells_the_network_to_drop_the_answers() {
        let (mut document, network) = fetching_page(r#"<script>fetch("a"); fetch("b");</script>"#);
        document.run_event_loop(0.0);
        assert_eq!(network.pending_count(), 2);

        document.cancel_all_tasks();
        assert_eq!(
            network.pending_count(),
            0,
            "the backend was told to stop delivering both"
        );
    }

    #[test]
    fn an_abort_reaches_the_network_on_the_next_turn() {
        let (mut document, network) = fetching_page(
            r#"<script>
                 const controller = new AbortController();
                 fetch("slow", { signal: controller.signal }).catch(function () {});
                 setTimeout(function () { controller.abort(); }, 10);
               </script>"#,
        );
        document.run_event_loop(0.0);
        assert_eq!(network.pending_count(), 1, "it is already on the wire");

        document.run_event_loop(10.0); // the timer aborts it
        document.run_event_loop(20.0); // the cancellation is passed on
        assert_eq!(
            network.pending_count(),
            0,
            "the backend was told the answer is no longer wanted"
        );
    }

    #[test]
    fn a_document_with_pending_requests_reports_that_it_needs_turns() {
        let (mut document, network) = fetching_page(r#"<script>fetch("a");</script>"#);
        assert!(document.has_pending_network());
        assert_eq!(document.next_wakeup_ms(), Some(0.0));

        document.run_event_loop(0.0);
        network.complete_all();
        document.run_event_loop(0.0);

        assert!(!document.has_pending_network());
        assert_eq!(
            document.next_wakeup_ms(),
            None,
            "with nothing outstanding the page is idle again"
        );
    }

    #[test]
    fn a_response_body_is_released_once_it_has_been_read() {
        // `bodyUsed` is the observable half of the ownership story: after a
        // read the bytes are gone, not merely flagged.
        let (mut document, network) = fetching_page(
            r#"<script>
                 fetch("big").then(function (r) {
                     r.text().then(function (t) {
                         console.log("read " + t.length + " bytes, bodyUsed " + r.bodyUsed);
                     });
                 });
               </script>"#,
        );
        network.respond_text("demo:///big", &"x".repeat(4096));
        run_fetches(&mut document, &network);

        assert_eq!(logs(&document), "read 4096 bytes, bodyUsed true");
    }

    // ── DOM and rendering ─────────────────────────────────────────────────

    #[test]
    fn a_fetch_can_build_dom_that_reaches_the_paint() {
        let (mut document, network) = fetching_page(
            r#"<ul id="rows"></ul>
               <script>
                 fetch("items.json")
                     .then(function (r) { return r.json(); })
                     .then(function (items) {
                         for (const item of items) {
                             const row = document.createElement("li");
                             row.textContent = item.label;
                             document.getElementById("rows").appendChild(row);
                         }
                     });
               </script>"#,
        );
        network.respond_json(
            "demo:///items.json",
            r#"[{"label":"first"},{"label":"second"}]"#,
        );

        let before = document
            .render(400, 300, 0.0, &PointerState::default())
            .to_ppm();
        run_fetches(&mut document, &network);

        assert_eq!(
            dom_api::query_selector_all(&document.dom, &[], "#rows li").len(),
            2
        );
        let after = document
            .render(400, 300, 0.0, &PointerState::default())
            .to_ppm();
        assert_ne!(before, after, "the fetched rows must be painted");
    }

    #[test]
    fn a_fetch_reaction_can_book_an_animation_frame() {
        let (mut document, network) = fetching_page(
            r#"<script>
                 fetch("x").then(function () {
                     console.log("reaction");
                     requestAnimationFrame(function () { console.log("frame"); });
                 });
               </script>"#,
        );
        network.respond_text("demo:///x", "ok");
        document.run_event_loop(0.0);
        network.complete_all();

        // The completion and its reaction land before this turn's frame stage,
        // so the frame it books runs in the same turn.
        let report = document.run_event_loop(16.0);
        assert_eq!(report.network_completions, 1);
        assert_eq!(report.frames_run, 1);
        assert_eq!(logs(&document), "reaction\nframe");
    }

    #[test]
    fn a_fetch_started_from_a_timer_works_the_same_way() {
        let (mut document, network) = fetching_page(
            r#"<script>
                 setTimeout(function () {
                     fetch("late").then(function (r) { console.log("late " + r.status); });
                 }, 50);
               </script>"#,
        );
        network.respond_text("demo:///late", "ok");

        document.run_event_loop(0.0);
        assert_eq!(
            document.in_flight_requests(),
            0,
            "the timer has not fired yet"
        );

        document.run_event_loop(50.0); // timer fires, request queued
        document.run_event_loop(50.0); // request sent
        network.complete_all();
        document.run_event_loop(50.0); // answer delivered

        assert_eq!(logs(&document), "late 200");
    }

    // ── Promise basics ────────────────────────────────────────────────────

    // ── Promise basics ────────────────────────────────────

    #[test]
    fn every_handler_on_one_promise_runs() {
        // Registering a handler does not consume the promise: all three fire,
        // in registration order, each with the same value.
        let document = page(
            r#"<script>
             const p = Promise.resolve("v");
             p.then(function (x) { console.log("one " + x); });
             p.then(function (x) { console.log("two " + x); });
             p.then(function (x) { console.log("three " + x); });
           </script>"#,
        );
        assert_eq!(logs(&document), "one v\ntwo v\nthree v");
    }

    #[test]
    fn each_then_returns_a_distinct_promise() {
        // Two branches off one promise do not interfere: the throw in the
        // first branch does not reach the second.
        let document = page(
            r#"<script>
             const p = Promise.resolve(1);
             p.then(function () { throw "branch A broke"; })
              .catch(function (e) { console.log("A: " + e); });
             p.then(function (x) { console.log("B: " + x); });
           </script>"#,
        );
        assert_eq!(logs(&document), "B: 1\nA: branch A broke");
    }

    #[test]
    fn a_long_chain_settles_without_growing_the_rust_stack() {
        // Draining is a loop, not recursion, so depth costs nothing. Each link
        // is one microtask, so a 2000-link chain also outruns the per-checkpoint
        // budget — which defers the remainder rather than dropping it.
        let mut document = page(
            r#"<script>
             let p = Promise.resolve(0);
             for (let i = 0; i < 2000; i++) {
                 p = p.then(function (x) { return x + 1; });
             }
             p.then(function (x) { console.log("depth " + x); });
           </script>"#,
        );

        let mut checkpoints = 1; // the one that ran after the script
        while document.has_pending_microtasks() {
            document.run_microtask_checkpoint();
            checkpoints += 1;
            assert!(checkpoints < 10, "the chain should not need many passes");
        }
        assert!(checkpoints > 1, "2000 links should exceed one checkpoint");
        assert!(
            logs(&document).ends_with("depth 2000"),
            "chain did not finish: {:?}",
            logs(&document)
        );
    }

    #[test]
    fn promise_any_rejects_only_when_every_entry_rejects() {
        let document = page(
            r#"<script>
             Promise.any([Promise.reject("a"), Promise.reject("b")])
                 .then(function () { console.log("unexpected fulfilment"); })
                 .catch(function (e) { console.log("rejected: " + e); });
           </script>"#,
        );
        assert_eq!(
            logs(&document),
            "rejected: AggregateError: all promises were rejected"
        );
    }

    #[test]
    fn a_settled_promise_keeps_its_first_outcome() {
        // The executor calls resolve twice and then reject; only the first
        // call counts, and the handler still runs asynchronously.
        let document = page(
            r#"<script>
             new Promise(function (resolve, reject) {
                 resolve("first");
                 resolve("second");
                 reject("nope");
             }).then(
                 function (v) { console.log("fulfilled " + v); },
                 function (e) { console.log("rejected " + e); }
             );
           </script>"#,
        );
        assert_eq!(logs(&document), "fulfilled first");
    }

    #[test]
    fn a_non_function_executor_throws_a_type_error() {
        let document = page(
            r#"<script>
             try { new Promise(5); } catch (e) { console.log("caught: " + e); }
           </script>"#,
        );
        assert!(
            logs(&document).contains("TypeError"),
            "expected a TypeError, got {:?}",
            logs(&document)
        );
    }

    #[test]
    fn constructing_a_non_constructor_throws() {
        // `new` is general, so this reports the same way for anything that is
        // not constructible.
        let document = page(
            r#"<script>
             try { new Nothing(); } catch (e) { console.log("caught: " + e); }
           </script>"#,
        );
        assert!(
            logs(&document).contains("caught:"),
            "expected the throw to be catchable, got {:?}",
            logs(&document)
        );
    }

    #[test]
    fn non_function_then_arguments_pass_the_value_through() {
        // `then(null)` is not a handler, so the value skips to the next link.
        let document = page(
            r#"<script>
             Promise.resolve("kept")
                 .then(null)
                 .then(7)
                 .then(function (v) { console.log(v); });
           </script>"#,
        );
        assert_eq!(logs(&document), "kept");
    }

    #[test]
    fn the_executor_runs_synchronously() {
        let document = page(
            r#"<script>
             console.log("A");
             new Promise(function (resolve) {
                 console.log("B");
                 resolve(42);
             });
             console.log("C");
           </script>"#,
        );
        assert_eq!(logs(&document), "A\nB\nC");
    }

    #[test]
    fn a_then_handler_never_runs_synchronously() {
        let document = page(
            r#"<script>
             const p = Promise.resolve(1);
             p.then(function () { console.log("then"); });
             console.log("sync");
           </script>"#,
        );
        assert_eq!(logs(&document), "sync\nthen");
    }

    #[test]
    fn a_promise_delivers_its_value_to_the_handler() {
        let document = page(
            r#"<script>
             Promise.resolve("hello").then(function (value) { console.log(value); });
             new Promise(function (resolve) { resolve("executor"); })
                 .then(function (value) { console.log(value); });
           </script>"#,
        );
        assert_eq!(logs(&document), "hello\nexecutor");
    }

    #[test]
    fn only_the_first_settlement_counts() {
        let document = page(
            r#"<script>
             new Promise(function (resolve, reject) {
                 resolve("first");
                 resolve("second");
                 reject("nope");
             }).then(
                 function (value) { console.log("fulfilled " + value); },
                 function (reason) { console.log("rejected " + reason); }
             );
           </script>"#,
        );
        assert_eq!(logs(&document), "fulfilled first");
    }

    #[test]
    fn every_registered_handler_receives_the_value() {
        let document = page(
            r#"<script>
             const p = Promise.resolve("v");
             p.then(function () { console.log("a"); });
             p.then(function () { console.log("b"); });
             p.then(function () { console.log("c"); });
           </script>"#,
        );
        assert_eq!(logs(&document), "a\nb\nc", "handlers are not consumed");
    }

    // ── Chaining ──────────────────────────────────────────────────────────

    #[test]
    fn values_flow_through_a_chain() {
        let document = page(
            r#"<p id="status">-</p>
           <script>
             Promise.resolve(1)
                 .then(function (x) { return x + 1; })
                 .then(function (x) { return x + 1; })
                 .then(function (x) {
                     document.getElementById("status").textContent = "" + x;
                 });
           </script>"#,
        );
        assert_eq!(text_of(&document, "#status"), "3");
    }

    #[test]
    fn a_missing_handler_passes_the_value_along() {
        let document = page(
            r#"<script>
             Promise.resolve("kept")
                 .then(undefined)
                 .then(function (value) { console.log(value); });
           </script>"#,
        );
        assert_eq!(logs(&document), "kept");
    }

    #[test]
    fn returning_a_promise_makes_the_chain_wait_for_it() {
        let mut document = page(
            r#"<script>
             Promise.resolve()
                 .then(function () {
                     return new Promise(function (resolve) {
                         setTimeout(function () { resolve("late"); }, 100);
                     });
                 })
                 .then(function (value) { console.log("got " + value); });
           </script>"#,
        );
        assert_eq!(logs(&document), "", "the chain is waiting on the timer");

        tick_at(&mut document, 50.0);
        assert_eq!(logs(&document), "");
        tick_at(&mut document, 100.0);
        assert_eq!(
            logs(&document),
            "got late",
            "settled in the timer's checkpoint"
        );
    }

    #[test]
    fn a_rejection_skips_fulfilment_handlers_until_a_catch() {
        let document = page(
            r#"<script>
             Promise.reject("bad")
                 .then(function () { console.log("skipped"); })
                 .catch(function (reason) { console.log("caught " + reason); });
           </script>"#,
        );
        assert_eq!(logs(&document), "caught bad");
    }

    #[test]
    fn catch_recovers_and_the_chain_continues() {
        let document = page(
            r#"<script>
             Promise.reject("bad")
                 .catch(function () { return "recovered"; })
                 .then(function (value) { console.log(value); });
           </script>"#,
        );
        assert_eq!(logs(&document), "recovered");
    }

    #[test]
    fn a_throwing_handler_rejects_the_next_promise() {
        let document = page(
            r#"<script>
             Promise.resolve()
                 .then(function () { throw "boom"; })
                 .catch(function (reason) { console.log("caught " + reason); });
           </script>"#,
        );
        assert_eq!(logs(&document), "caught boom");
    }

    #[test]
    fn a_runtime_error_in_a_handler_becomes_a_rejection() {
        let document = page(
            r#"<p id="status">-</p>
           <script>
             Promise.resolve()
                 .then(function () { nonexistent.foo(); })
                 .catch(function () {
                     document.getElementById("status").textContent = "caught";
                 });
           </script>"#,
        );
        assert_eq!(text_of(&document, "#status"), "caught");
    }

    #[test]
    fn throwing_in_an_executor_rejects_the_promise() {
        let document = page(
            r#"<script>
             new Promise(function () { throw "executor failed"; })
                 .catch(function (reason) { console.log("caught " + reason); });
           </script>"#,
        );
        assert_eq!(logs(&document), "caught executor failed");
    }

    #[test]
    fn resolving_a_promise_with_itself_rejects_instead_of_hanging() {
        let document = page(
            r#"<script>
             let p2;
             p2 = Promise.resolve().then(function () { return p2; });
             p2.catch(function (reason) { console.log("" + reason); });
           </script>"#,
        );
        assert!(
            logs(&document).contains("chaining cycle"),
            "expected a cycle rejection, got {:?}",
            logs(&document)
        );
    }

    // ── finally ───────────────────────────────────────────────────────────

    #[test]
    fn finally_passes_the_fulfilment_value_through() {
        let document = page(
            r#"<script>
             Promise.resolve("value")
                 .finally(function () { console.log("cleanup"); })
                 .then(function (value) { console.log("got " + value); });
           </script>"#,
        );
        assert_eq!(logs(&document), "cleanup\ngot value");
    }

    #[test]
    fn finally_passes_a_rejection_through() {
        let document = page(
            r#"<script>
             Promise.reject("bad")
                 .finally(function () { console.log("cleanup"); })
                 .catch(function (reason) { console.log("caught " + reason); });
           </script>"#,
        );
        assert_eq!(logs(&document), "cleanup\ncaught bad");
    }

    #[test]
    fn a_throwing_finally_replaces_the_outcome() {
        let document = page(
            r#"<script>
             Promise.resolve("value")
                 .finally(function () { throw "cleanup failed"; })
                 .catch(function (reason) { console.log("caught " + reason); });
           </script>"#,
        );
        assert_eq!(logs(&document), "caught cleanup failed");
    }

    // ── Static methods ────────────────────────────────────────────────────

    #[test]
    fn promise_resolve_passes_an_existing_promise_through() {
        let document = page(
            r#"<script>
             const inner = Promise.resolve("x");
             console.log("" + (Promise.resolve(inner) === inner));
           </script>"#,
        );
        assert_eq!(logs(&document), "true");
    }

    #[test]
    fn promise_all_keeps_the_input_order() {
        let mut document = page(
            r#"<script>
             const slow = new Promise(function (resolve) {
                 setTimeout(function () { resolve("slow"); }, 100);
             });
             const fast = Promise.resolve("fast");
             Promise.all([slow, fast, "plain"]).then(function (values) {
                 console.log(values.join(","));
             });
           </script>"#,
        );
        assert_eq!(logs(&document), "", "waiting for the slow entry");
        tick_at(&mut document, 100.0);
        assert_eq!(logs(&document), "slow,fast,plain");
    }

    #[test]
    fn promise_all_rejects_on_the_first_rejection() {
        let document = page(
            r#"<script>
             Promise.all([Promise.resolve(1), Promise.reject("bad"), Promise.resolve(3)])
                 .then(function () { console.log("should not happen"); })
                 .catch(function (reason) { console.log("caught " + reason); });
           </script>"#,
        );
        assert_eq!(logs(&document), "caught bad");
    }

    #[test]
    fn promise_all_of_an_empty_array_fulfils_immediately() {
        let document = page(
            r#"<script>
             Promise.all([]).then(function (values) {
                 console.log("empty:" + values.length);
             });
           </script>"#,
        );
        assert_eq!(logs(&document), "empty:0");
    }

    #[test]
    fn promise_race_takes_the_first_settlement() {
        let mut document = page(
            r#"<script>
             const slow = new Promise(function (resolve) {
                 setTimeout(function () { resolve("slow"); }, 100);
             });
             const quick = new Promise(function (resolve) {
                 setTimeout(function () { resolve("quick"); }, 10);
             });
             Promise.race([slow, quick]).then(function (value) { console.log(value); });
           </script>"#,
        );
        tick_at(&mut document, 10.0);
        assert_eq!(logs(&document), "quick");
        tick_at(&mut document, 100.0);
        assert_eq!(logs(&document), "quick", "the loser cannot settle it again");
    }

    #[test]
    fn promise_all_settled_reports_every_outcome() {
        let document = page(
            r#"<script>
             Promise.allSettled([Promise.resolve("ok"), Promise.reject("bad")])
                 .then(function (results) {
                     console.log(results[0].status + ":" + results[0].value);
                     console.log(results[1].status + ":" + results[1].reason);
                 });
           </script>"#,
        );
        assert_eq!(logs(&document), "fulfilled:ok\nrejected:bad");
    }

    #[test]
    fn promise_any_takes_the_first_fulfilment() {
        let document = page(
            r#"<script>
             Promise.any([Promise.reject("no"), Promise.resolve("yes")])
                 .then(function (value) { console.log(value); });
             Promise.any([Promise.reject("a"), Promise.reject("b")])
                 .catch(function (reason) { console.log("" + reason); });
           </script>"#,
        );
        let output = logs(&document);
        assert!(output.contains("yes"), "{output}");
        assert!(output.contains("AggregateError"), "{output}");
    }

    // ── throw / try / catch ───────────────────────────────────────────────

    #[test]
    fn try_catch_handles_a_thrown_value() {
        let document = page(
            r#"<script>
             try {
                 throw "oops";
                 console.log("unreachable");
             } catch (error) {
                 console.log("caught " + error);
             }
             console.log("after");
           </script>"#,
        );
        assert_eq!(logs(&document), "caught oops\nafter");
    }

    #[test]
    fn finally_runs_whether_or_not_something_was_thrown() {
        let document = page(
            r#"<script>
             try {
                 throw "x";
             } catch (e) {
                 console.log("catch");
             } finally {
                 console.log("finally");
             }

             try {
                 console.log("body");
             } finally {
                 console.log("cleanup");
             }
           </script>"#,
        );
        assert_eq!(logs(&document), "catch\nfinally\nbody\ncleanup");
    }

    #[test]
    fn an_uncaught_throw_unwinds_out_of_calls_and_loops() {
        let document = page(
            r#"<script>
             function inner() { throw "deep"; }
             function outer() {
                 for (let i = 0; i < 10; i++) {
                     inner();
                     console.log("not reached");
                 }
             }
             try {
                 outer();
             } catch (error) {
                 console.log("caught " + error);
             }
           </script>"#,
        );
        assert_eq!(logs(&document), "caught deep");
    }

    #[test]
    fn an_uncaught_exception_is_reported_and_the_page_survives() {
        let mut document = page(
            r#"<p id="status">alive</p>
           <script>
             throw "unhandled";
           </script>"#,
        );
        assert!(
            logs(&document).contains("Uncaught unhandled"),
            "{}",
            logs(&document)
        );
        assert_eq!(text_of(&document, "#status"), "alive");

        document
            .runtime
            .run_script(&mut document.dom, r#"console.log("next script runs");"#);
        assert!(logs(&document).contains("next script runs"));
    }

    #[test]
    fn a_runtime_error_can_be_caught_by_try_catch() {
        let document = page(
            r#"<script>
             try {
                 nonexistent.foo();
             } catch (error) {
                 console.log("caught: " + error);
             }
           </script>"#,
        );
        assert!(
            logs(&document).starts_with("caught: TypeError"),
            "{}",
            logs(&document)
        );
    }

    // ── DOM integration ───────────────────────────────────────────────────

    #[test]
    fn a_promise_chain_drives_the_dom_across_a_timer() {
        let mut document = page(
            r#"<p id="status">start</p>
           <script>
             Promise.resolve()
                 .then(function () {
                     document.getElementById("status").textContent = "phase 1";
                 })
                 .then(function () {
                     return new Promise(function (resolve) {
                         setTimeout(resolve, 100);
                     });
                 })
                 .then(function () {
                     document.getElementById("status").textContent = "done";
                 });
           </script>"#,
        );
        assert_eq!(text_of(&document, "#status"), "phase 1");

        tick_at(&mut document, 99.0);
        assert_eq!(text_of(&document, "#status"), "phase 1");
        tick_at(&mut document, 100.0);
        assert_eq!(text_of(&document, "#status"), "done");
    }

    #[test]
    fn a_promise_can_build_dom_that_reaches_the_paint() {
        let document = page(
            r#"<ul id="list"></ul>
           <script>
             Promise.resolve([1, 2, 3]).then(function (rows) {
                 const list = document.getElementById("list");
                 for (const row of rows) {
                     const item = document.createElement("li");
                     item.textContent = "row " + row;
                     list.appendChild(item);
                 }
             });
           </script>"#,
        );
        assert_eq!(
            dom_api::query_selector_all(&document.dom, &[], "li").len(),
            3
        );

        let canvas = document.render(300, 150, 0.0, &PointerState::default());
        assert_eq!(canvas.width, 300);
    }

    #[test]
    fn a_promise_can_change_style_and_the_paint_follows() {
        let mut document = page(
            r#"<div id="box" style="width: 40px; height: 40px; background-color: rgb(20, 20, 20)"></div>
           <script>
             const box = document.getElementById("box");
             setTimeout(function () {
                 Promise.resolve().then(function () {
                     box.style.backgroundColor = "rgb(220, 40, 40)";
                 });
             }, 10);
           </script>"#,
        );
        let before = document
            .render(100, 60, 0.0, &PointerState::default())
            .to_ppm();

        tick_at(&mut document, 10.0);
        let after = document
            .render(100, 60, 0.0, &PointerState::default())
            .to_ppm();
        assert_ne!(
            before, after,
            "the promise-driven style change must be painted"
        );
    }

    #[test]
    fn a_promise_can_schedule_a_timer_and_a_frame() {
        let mut document = page(
            r#"<script>
             Promise.resolve().then(function () {
                 setTimeout(function () { console.log("timer from promise"); }, 10);
                 requestAnimationFrame(function () { console.log("frame from promise"); });
             });
           </script>"#,
        );
        assert!(document.has_pending_tasks(), "the microtask scheduled work");

        tick_at(&mut document, 10.0);
        let output = logs(&document);
        assert!(output.contains("timer from promise"), "{output}");
        assert!(output.contains("frame from promise"), "{output}");
    }

    #[test]
    fn an_event_listener_can_start_a_promise_chain() {
        let mut document = page(
            r#"<button id="go">go</button><p id="status">idle</p>
           <script>
             document.getElementById("go").addEventListener("click", function () {
                 Promise.resolve()
                     .then(function () {
                         document.getElementById("status").textContent = "working";
                     });
             });
           </script>"#,
        );
        let go = path_of(&document, "#go");
        document
            .runtime
            .dispatch_event(&mut document.dom, &go, "click");
        document.run_microtask_checkpoint();
        assert_eq!(text_of(&document, "#status"), "working");
    }
}
