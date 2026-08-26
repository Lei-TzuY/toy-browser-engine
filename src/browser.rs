// ============================================================
//  browser.rs  —  Navigation, history and input handling
// ============================================================
//
//  A `Browser` owns the resource loader, the session history and the document
//  currently on screen. It is the layer a UI drives: hand it clicks and
//  navigation commands, ask it for a canvas.
//
//  History is the classic model: a list of visited URLs and an index into it.
//  Navigating truncates everything after the current entry; back and forward
//  move the index and reload that entry.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use crate::document::{Document, LoopReport, PageAction, PointerState};
use crate::eventloop::{Clock, RealClock};
use crate::forms::{self, encode_form_entries, Submission, SubmissionMethod};
use crate::input::{Key, KeyEvent};
use crate::net::{
    DefaultNetwork, FetchError, FetchRequest, HeaderMap, LoadError, Method, NetworkBackend,
    ResourceLoader, Url,
};
use crate::script::interp::{EventInit, JsValue};
use crate::script::NodePath;
use crate::validation;

/// How long one idle turn of [`Browser::settle_network`] waits on the network
/// before giving the loop another go.
const NETWORK_WAIT: Duration = Duration::from_millis(250);

/// What happened when a click was delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClickOutcome {
    /// Nothing was under the pointer, or nothing responded.
    Ignored,
    /// A script handled the click (and may have changed the DOM).
    Script,
    /// A link was followed to `url`.
    Navigated(Url),
    /// A link was followed but the target could not be loaded.
    NavigationFailed { url: Url, error: LoadError },
}

pub struct Browser {
    /// Shared with the network, which may hand it to a worker thread — hence
    /// `Arc` rather than `Box`. The public constructors still take a `Box`, so
    /// nothing outside has to know.
    loader: Arc<dyn ResourceLoader>,
    /// Where `fetch()` goes. One per session, shared by every document it
    /// shows; a completion for a page that has been navigated away from is
    /// dropped because its registry is gone, not because the pipe was.
    network: Rc<dyn NetworkBackend>,
    /// Visited URLs, oldest first.
    history: Vec<Url>,
    /// Index into `history` of the entry on screen.
    index: usize,
    document: Document,
    /// Drives the event loop. Real time in a window, virtual time in tests.
    clock: Rc<dyn Clock>,
    /// Loop time at which the current document started, so `performance.now()`
    /// and frame timestamps are measured from page load.
    document_epoch_ms: f64,
}

impl Browser {
    /// Load `url` and start a session at it, driven by the real clock.
    pub fn open(loader: Box<dyn ResourceLoader>, url: &Url) -> Result<Browser, LoadError> {
        Browser::open_with_clock(loader, url, Rc::new(RealClock::new()))
    }

    /// Load `url` with a caller-supplied clock.
    ///
    /// Tests pass a [`ManualClock`](crate::eventloop::ManualClock) so time only
    /// moves when they say so, which is what makes timer tests deterministic
    /// without sleeping.
    pub fn open_with_clock(
        loader: Box<dyn ResourceLoader>,
        url: &Url,
        clock: Rc<dyn Clock>,
    ) -> Result<Browser, LoadError> {
        let loader: Arc<dyn ResourceLoader> = Arc::from(loader);
        let network = Rc::new(DefaultNetwork::new(loader.clone()));
        Browser::open_with(loader, network, url, clock)
    }

    /// Load `url` with a caller-supplied clock *and* network.
    ///
    /// This is the deterministic entry point: hand it a
    /// [`ManualNetwork`](crate::net::ManualNetwork) and no request completes
    /// until the caller says so, which is what lets a fetch test assert that a
    /// promise is still pending.
    pub fn open_with_network(
        loader: Box<dyn ResourceLoader>,
        network: Rc<dyn NetworkBackend>,
        url: &Url,
        clock: Rc<dyn Clock>,
    ) -> Result<Browser, LoadError> {
        Browser::open_with(Arc::from(loader), network, url, clock)
    }

    fn open_with(
        loader: Arc<dyn ResourceLoader>,
        network: Rc<dyn NetworkBackend>,
        url: &Url,
        clock: Rc<dyn Clock>,
    ) -> Result<Browser, LoadError> {
        let mut document = Document::load(url, loader.as_ref())?;
        document.set_network(network.clone());
        let epoch = clock.now_ms();
        Ok(Browser {
            loader,
            network,
            history: vec![document.url.clone()],
            index: 0,
            document,
            clock,
            document_epoch_ms: epoch,
        })
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn document_mut(&mut self) -> &mut Document {
        &mut self.document
    }

    pub fn loader(&self) -> &dyn ResourceLoader {
        self.loader.as_ref()
    }

    /// The URL on screen, including any fragment the user navigated to.
    pub fn url(&self) -> &Url {
        &self.history[self.index]
    }

    pub fn history(&self) -> &[Url] {
        &self.history
    }

    pub fn history_index(&self) -> usize {
        self.index
    }

    pub fn can_go_back(&self) -> bool {
        self.index > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.index + 1 < self.history.len()
    }

    /// Navigate to `url`, pushing a history entry.
    ///
    /// A URL that differs only by fragment does not refetch the document, the
    /// same way a real browser jumps within the current page.
    pub fn navigate(&mut self, url: &Url) -> Result<(), LoadError> {
        if !url.same_document(self.url()) {
            self.replace_document(Document::load(url, self.loader.as_ref())?);
        }
        self.history.truncate(self.index + 1);
        self.history.push(url.clone());
        self.index = self.history.len() - 1;
        Ok(())
    }

    /// Resolve `href` against the current document and navigate to it.
    pub fn follow_link(&mut self, href: &str) -> Result<(), LoadError> {
        let url = self
            .document
            .resolve(href)
            .ok_or_else(|| LoadError::InvalidUrl(href.to_string()))?;
        self.navigate(&url)
    }

    /// Reload the current entry from its source.
    pub fn reload(&mut self) -> Result<(), LoadError> {
        let url = self.url().clone();
        self.replace_document(Document::load(&url, self.loader.as_ref())?);
        Ok(())
    }

    /// Put a freshly loaded document on screen.
    ///
    /// The outgoing document — and with it every timer, interval and
    /// animation-frame callback its scripts registered — is dropped here, so a
    /// page that has been navigated away from cannot keep running.
    fn replace_document(&mut self, mut document: Document) {
        // Cancelling first drops the outgoing page's fetch registry, and with
        // it every pending promise: an answer that arrives afterwards has
        // nothing left to settle and is discarded.
        self.document.cancel_all_tasks();
        document.set_network(self.network.clone());
        self.document = document;
        self.document_epoch_ms = self.clock.now_ms();
    }

    /// Step back in history. Returns false at the start of the session.
    pub fn back(&mut self) -> bool {
        if !self.can_go_back() {
            return false;
        }
        self.index -= 1;
        self.restore_current_entry();
        true
    }

    /// Step forward in history. Returns false at the most recent entry.
    pub fn forward(&mut self) -> bool {
        if !self.can_go_forward() {
            return false;
        }
        self.index += 1;
        self.restore_current_entry();
        true
    }

    /// Reload the entry at the current index (no back/forward cache, so a
    /// revisited page starts fresh).
    fn restore_current_entry(&mut self) {
        let url = self.history[self.index].clone();
        if url.same_document(&self.document.url) {
            return;
        }
        match Document::load(&url, self.loader.as_ref()) {
            Ok(document) => self.replace_document(document),
            Err(error) => self.document.diagnostics.push(crate::document::Diagnostic {
                url: url.to_string(),
                message: error.to_string(),
            }),
        }
    }

    /// Deliver a click at a DOM node: scripts first, then link activation.
    ///
    /// A handler that calls `preventDefault()` suppresses the navigation, as
    /// in a real browser.
    pub fn click_node(&mut self, path: &NodePath) -> ClickOutcome {
        // Focus moves on press, before the click event — that is the order a
        // handler sees when it reads `document.activeElement`.
        self.document.focus_from_click(path);

        let outcome = {
            let document = &mut self.document;
            document
                .runtime
                .dispatch_event(&mut document.dom, path, "click")
        };
        let submitted = self.document.apply_pending_actions();
        // A listener is a task like any other, so its microtasks run now
        // rather than waiting for the next turn of the loop: a promise
        // resolved in a click handler settles before the click returns.
        self.document.run_microtask_checkpoint();
        // Handlers may have added images or rewritten the page.
        if outcome.dispatched {
            self.document.refresh_images(self.loader.as_ref());
        }
        if let Some(submission) = submitted {
            // `form.submit()` is the programmatic path: by design it bypasses
            // interactive constraint validation as well as the submit event.
            return self.perform_submission(submission);
        }

        if outcome.default_prevented {
            return ClickOutcome::Script;
        }

        // Preserve the activated submitter through validation and the form's
        // submit event. A submit listener may edit values or override
        // attributes, so the final Submission is rebuilt from the live DOM
        // after activation rather than trusting Document's no-submitter one.
        let submit_context = self.submitter_for_activation(path);
        if let Some((form_path, submitter_path)) = submit_context.as_ref() {
            let skips_validation = forms::submission_skips_validation(
                &self.document.dom,
                form_path,
                Some(submitter_path),
            );
            if !skips_validation && !self.report_constraint_violations(form_path) {
                return ClickOutcome::Script;
            }
        }

        // Default action: activate the control, then follow a link.
        if let PageAction::Submit(fallback) = self.document.activate(path) {
            let submission = submit_context
                .and_then(|(form_path, submitter_path)| {
                    forms::prepare_submission_with_submitter(
                        &self.document.dom,
                        &form_path,
                        Some(&submitter_path),
                        &self.document.base_url,
                    )
                })
                .unwrap_or(fallback);
            return self.perform_submission(submission);
        }
        match self.document.link_at(path) {
            Some(url) => match self.navigate(&url) {
                Ok(()) => ClickOutcome::Navigated(url),
                Err(error) => ClickOutcome::NavigationFailed { url, error },
            },
            None if outcome.dispatched => ClickOutcome::Script,
            None => ClickOutcome::Ignored,
        }
    }

    // ── Event loop ────────────────────────────────────────────────────────

    /// Loop time, in milliseconds since the current document was loaded.
    pub fn now_ms(&self) -> f64 {
        (self.clock.now_ms() - self.document_epoch_ms).max(0.0)
    }

    /// Run one turn of the event loop at the current time.
    ///
    /// This is step 3–4 of a frame: due timers, then animation-frame
    /// callbacks. The caller renders afterwards, so anything a callback
    /// changed is in the next paint.
    pub fn tick(&mut self) -> LoopReport {
        let now = self.now_ms();
        let mut report = self.document.run_event_loop(now);
        if let Some(submission) = report.submission.take() {
            // A callback called `form.submit()`; carry it out like any other.
            self.perform_submission(submission);
        }
        report
    }

    /// Move a manual clock forward and run a turn.
    ///
    /// With the real clock this simply ticks — time advances on its own.
    pub fn advance_time(&mut self, delta: Duration) -> LoopReport {
        self.clock.advance_ms(delta.as_secs_f64() * 1000.0);
        self.tick()
    }

    /// Advance in fixed steps, running a turn after each one.
    ///
    /// Timers only fire when the loop actually runs, so stepping is how a test
    /// gives an interval the chances to fire that a real frame cadence would.
    pub fn advance_time_in_steps(&mut self, total: Duration, step: Duration) -> LoopReport {
        let mut combined = LoopReport::default();
        let step_ms = step.as_secs_f64().max(0.001) * 1000.0;
        let mut remaining = total.as_secs_f64() * 1000.0;
        while remaining > 0.0 {
            let slice = step_ms.min(remaining);
            let report = self.advance_time(Duration::from_secs_f64(slice / 1000.0));
            combined.timers_run += report.timers_run;
            combined.frames_run += report.frames_run;
            combined.network_completions += report.network_completions;
            combined.requests_sent += report.requests_sent;
            remaining -= slice;
        }
        combined
    }

    /// When the page next needs a turn, in milliseconds from now.
    ///
    /// `Some(0)` means "right away", `None` means the page is idle and the
    /// driver can block on input instead of spinning.
    pub fn next_wakeup_in_ms(&self) -> Option<f64> {
        let now = self.now_ms();
        self.document
            .next_wakeup_ms()
            .map(|deadline| (deadline - now).max(0.0))
    }

    /// True while the page has timers, frame callbacks or requests outstanding.
    pub fn has_pending_tasks(&self) -> bool {
        self.document.has_pending_tasks()
    }

    /// The network this session fetches through.
    pub fn network(&self) -> &Rc<dyn NetworkBackend> {
        &self.network
    }

    /// Turn the loop until nothing is outstanding, at the current time.
    ///
    /// A request needs one turn to be sent and another to be collected, and
    /// its reactions may start more work, so a caller that just wants the
    /// network to settle would otherwise have to guess how many ticks to run.
    ///
    /// When a turn finds nothing to do but something is still in flight, this
    /// blocks on the network rather than spinning — the same thing an idle
    /// event loop does, and the reason a socket-backed fetch does not need a
    /// sleep to be waited for. `limit` bounds the number of turns, so a page
    /// that fetches in a loop cannot hang a driver.
    pub fn settle_network(&mut self, limit: usize) -> LoopReport {
        let mut combined = LoopReport::default();
        for _ in 0..limit {
            if !self.document.has_pending_network() {
                break;
            }
            let report = self.tick();
            let idle = !report.did_work() && report.requests_sent == 0;
            combined.timers_run += report.timers_run;
            combined.frames_run += report.frames_run;
            combined.network_completions += report.network_completions;
            combined.requests_sent += report.requests_sent;

            if idle && self.document.has_pending_network() {
                // Nothing to run and an answer still owed: wait for it. A wait
                // that times out only means "not yet" — a slow connection can
                // take several. It is a backend with no work in progress that
                // is never going to deliver, and only then is turning the loop
                // again pointless.
                if !self.network.wait(NETWORK_WAIT) && !self.network.is_busy() {
                    break;
                }
            }
        }
        combined
    }

    // ── Keyboard ──────────────────────────────────────────────────────────

    /// Deliver a key press to the focused element, performing whatever default
    /// action survives the `keydown` listeners.
    pub fn key_down(&mut self, event: &KeyEvent) -> ClickOutcome {
        // Single-line Enter submission needs validation *before* the submit
        // event, so Browser owns this default action instead of asking
        // Document::key_down to combine both steps. Every other key stays on
        // the normal Document path.
        if let Some((target, form_path, submitter)) = self.implicit_submission_context(event) {
            return self.perform_implicit_key_submission(event, target, form_path, submitter);
        }

        let action = self.document.key_down(event);
        self.after_document_action(action)
    }

    /// Deliver the matching release.
    pub fn key_up(&mut self, event: &KeyEvent) {
        self.document.key_up(event);
        self.document.apply_pending_actions();
    }

    /// A complete press and release.
    pub fn press_key(&mut self, event: &KeyEvent) -> ClickOutcome {
        let outcome = self.key_down(event);
        // A navigation replaced the document; the release belongs to the old one.
        if matches!(outcome, ClickOutcome::Navigated(_)) {
            return outcome;
        }
        self.key_up(event);
        outcome
    }

    /// Type a string into the focused control, one key at a time.
    pub fn type_text(&mut self, text: &str) {
        for character in text.chars() {
            self.press_key(&KeyEvent::character(character));
        }
    }

    /// Submit a form as if its submit button had been pressed.
    pub fn submit_form(&mut self, form_path: &NodePath) -> ClickOutcome {
        let skips_validation =
            forms::submission_skips_validation(&self.document.dom, form_path, None);
        if !skips_validation && !self.report_constraint_violations(form_path) {
            return ClickOutcome::Script;
        }
        let action = self.document.submit_form(form_path);
        self.after_document_action(action)
    }

    /// Carry out whatever the document asked the session to do.
    fn after_document_action(&mut self, action: PageAction) -> ClickOutcome {
        match action {
            PageAction::None => {
                if let Some(submission) = self.document.apply_pending_actions() {
                    // Pending submissions are `form.submit()` and therefore
                    // deliberately bypass interactive validation.
                    return self.perform_submission(submission);
                }
                ClickOutcome::Script
            }
            PageAction::Submit(submission) => self.perform_submission(submission),
        }
    }

    /// Return the owning form and the actual submit control for a click.
    fn submitter_for_activation(&self, target: &[usize]) -> Option<(NodePath, NodePath)> {
        let element = crate::script::dom_api::node_at(&self.document.dom, target)?.as_element()?;
        if !forms::is_submit_control(element) {
            return None;
        }
        let form = forms::owning_form(&self.document.dom, target)?;
        Some((form, target.to_vec()))
    }

    /// Recognise the Enter default action that performs implicit form submit.
    fn implicit_submission_context(
        &self,
        event: &KeyEvent,
    ) -> Option<(NodePath, NodePath, Option<NodePath>)> {
        if event.key != Key::Enter {
            return None;
        }
        let target = self.document.focused_path()?;
        let element = crate::script::dom_api::node_at(&self.document.dom, &target)?.as_element()?;
        if element.tag_name != "input" || !element.is_text_entry() {
            return None;
        }
        let form = forms::owning_form(&self.document.dom, &target)?;
        if !forms::allows_implicit_submission(&self.document.dom, &form) {
            return None;
        }
        let submitter = forms::implicit_submitter(&self.document.dom, &form);
        Some((target, form, submitter))
    }

    /// Run the Enter submission default action in standards order:
    /// keydown → validation → submit → navigation.
    fn perform_implicit_key_submission(
        &mut self,
        event: &KeyEvent,
        target: NodePath,
        form_path: NodePath,
        submitter: Option<NodePath>,
    ) -> ClickOutcome {
        let key_outcome = {
            let document = &mut self.document;
            document.runtime.dispatch_event_init(
                &mut document.dom,
                &target,
                "keydown",
                browser_key_event_init(event),
            )
        };
        let requested = self.document.apply_pending_actions();
        self.document.run_microtask_checkpoint();
        if let Some(submission) = requested {
            // A key listener explicitly called `form.submit()`; that API is
            // programmatic and bypasses interactive validation.
            return self.perform_submission(submission);
        }
        if key_outcome.default_prevented {
            return ClickOutcome::Script;
        }

        let skips_validation = forms::submission_skips_validation(
            &self.document.dom,
            &form_path,
            submitter.as_deref(),
        );
        if !skips_validation && !self.report_constraint_violations(&form_path) {
            return ClickOutcome::Script;
        }

        let submit_outcome = {
            let document = &mut self.document;
            document.runtime.dispatch_event_init(
                &mut document.dom,
                &form_path,
                "submit",
                EventInit::bubbling(),
            )
        };
        let requested = self.document.apply_pending_actions();
        self.document.run_microtask_checkpoint();
        if let Some(submission) = requested {
            return self.perform_submission(submission);
        }
        if submit_outcome.default_prevented {
            return ClickOutcome::Script;
        }

        match forms::prepare_submission_with_submitter(
            &self.document.dom,
            &form_path,
            submitter.as_deref(),
            &self.document.base_url,
        ) {
            Some(submission) => self.perform_submission(submission),
            None => ClickOutcome::Script,
        }
    }

    /// Run interactive constraint validation for `form_path`.
    ///
    /// `invalid` does not bubble. Every failing control receives it in document
    /// order, then the first invalid control receives focus. Returning `false`
    /// tells the caller to suppress the submission's default action.
    fn report_constraint_violations(&mut self, form_path: &[usize]) -> bool {
        let invalid = validation::invalid_controls(&self.document.dom, form_path);
        if invalid.is_empty() {
            return true;
        }

        for path in &invalid {
            let document = &mut self.document;
            document.runtime.dispatch_event_init(
                &mut document.dom,
                path,
                "invalid",
                EventInit::non_bubbling(),
            );
            document.apply_pending_actions();
            document.run_microtask_checkpoint();
        }
        if let Some(first) = invalid.first() {
            self.document.focus_path(first);
        }
        false
    }

    /// Navigate to a prepared form submission.
    fn perform_submission(&mut self, submission: Submission) -> ClickOutcome {
        match submission.method {
            SubmissionMethod::Get => match self.navigate(&submission.url) {
                Ok(()) => ClickOutcome::Navigated(submission.url),
                Err(error) => ClickOutcome::NavigationFailed {
                    url: submission.url,
                    error,
                },
            },
            SubmissionMethod::Post => self.perform_post_submission(submission),
        }
    }

    /// Submit a URL-encoded form body and navigate to the HTML response.
    ///
    /// This deliberately goes through the same `ResourceLoader::fetch` path
    /// JavaScript `fetch()` uses, so HTTP method/body/redirect handling stays
    /// in one transport implementation. Static loaders answer POST with 405;
    /// real HTTP loaders put the request on the wire.
    fn perform_post_submission(&mut self, submission: Submission) -> ClickOutcome {
        let body = encode_form_entries(&submission.entries).into_bytes();
        let mut headers = HeaderMap::new();
        headers.insert_raw(
            "content-type",
            "application/x-www-form-urlencoded; charset=UTF-8",
        );
        let request = FetchRequest::new(
            submission.url.clone(),
            Method::Post,
            headers,
            Some(body),
        );

        match self.loader.fetch(&request) {
            Ok(response) if response.ok() => {
                let final_url = response.url.clone();
                let html = String::from_utf8_lossy(&response.body);
                let document = Document::from_html(&html, &final_url, self.loader.as_ref());
                self.replace_document(document);
                self.history.truncate(self.index + 1);
                self.history.push(final_url.clone());
                self.index = self.history.len() - 1;
                ClickOutcome::Navigated(final_url)
            }
            Ok(response) => ClickOutcome::NavigationFailed {
                url: response.url.clone(),
                error: LoadError::HttpStatus {
                    url: response.url.to_string(),
                    status: response.status,
                },
            },
            Err(error) => ClickOutcome::NavigationFailed {
                url: submission.url.clone(),
                error: load_error_from_fetch(&submission.url, error),
            },
        }
    }

    /// Hit-test a point in page coordinates and deliver a click there.
    pub fn click_at(&mut self, x: f32, y: f32, viewport_width: f32) -> ClickOutcome {
        match self.document.hit_test(x, y, viewport_width) {
            Some(path) => self.click_node(&path),
            None => ClickOutcome::Ignored,
        }
    }

    /// One-line description of the session for a title bar.
    pub fn status_line(&self) -> String {
        let position = format!("{}/{}", self.index + 1, self.history.len());
        match self.document.title() {
            Some(title) => format!("{title} — {} [{position}]", self.url()),
            None => format!("{} [{position}]", self.url()),
        }
    }

    /// Render the current document.
    pub fn render(
        &self,
        width: usize,
        height: usize,
        scroll_y: f32,
        pointer: &PointerState,
    ) -> crate::paint::Canvas {
        self.document.render(width, height, scroll_y, pointer)
    }
}

fn browser_key_event_init(event: &KeyEvent) -> EventInit {
    EventInit::bubbling()
        .with_field("key", JsValue::Str(event.key.key_value()))
        .with_field("code", JsValue::Str(event.key.code_value()))
        .with_field("shiftKey", JsValue::Bool(event.modifiers.shift))
        .with_field("ctrlKey", JsValue::Bool(event.modifiers.ctrl))
        .with_field("altKey", JsValue::Bool(event.modifiers.alt))
}

fn load_error_from_fetch(url: &Url, error: FetchError) -> LoadError {
    match error {
        FetchError::InvalidUrl(text) => LoadError::InvalidUrl(text),
        FetchError::UnsupportedScheme(scheme) => LoadError::UnsupportedScheme(scheme),
        FetchError::TooManyRedirects(target) => LoadError::TooManyRedirects(target),
        other => LoadError::Io {
            url: url.to_string(),
            message: other.to_string(),
        },
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eventloop::ManualClock;
    use crate::input::Key;
    use crate::net::MemoryLoader;
    use crate::script::dom_api;

    fn site() -> MemoryLoader {
        let mut loader = MemoryLoader::new();
        loader.insert(
            "demo:///index.html",
            // `r##"…"##`: the fragment link below contains `"#`, which would
            // close a plain raw string.
            r##"<title>Home</title>
               <p><a id="to-about" href="pages/about.html">About</a></p>
               <p><a id="to-anchor" href="#section">Jump</a></p>
               <button id="btn">count</button>
               <script>
                 let clicks = 0;
                 document.getElementById("btn").addEventListener("click", function () {
                     clicks++;
                     document.getElementById("btn").textContent = "clicks: " + clicks;
                 });
               </script>"##,
        );
        loader.insert(
            "demo:///pages/about.html",
            r#"<title>About</title><p><a id="home" href="../index.html">Home</a></p>"#,
        );
        loader
    }

    fn browser() -> Browser {
        Browser::open(Box::new(site()), &Url::parse("demo:///index.html").unwrap())
            .expect("session opens")
    }

    #[test]
    fn opens_at_the_requested_url() {
        let browser = browser();
        assert_eq!(browser.url().to_string(), "demo:///index.html");
        assert_eq!(browser.document().title().as_deref(), Some("Home"));
        assert!(!browser.can_go_back() && !browser.can_go_forward());
    }

    #[test]
    fn following_a_relative_link_navigates_and_records_history() {
        let mut browser = browser();
        browser.follow_link("pages/about.html").expect("navigates");
        assert_eq!(browser.url().to_string(), "demo:///pages/about.html");
        assert_eq!(browser.document().title().as_deref(), Some("About"));
        assert!(browser.can_go_back());
        assert_eq!(browser.history().len(), 2);
    }

    #[test]
    fn clicking_a_link_element_navigates() {
        let mut browser = browser();
        let link = dom_api::get_element_by_id(&browser.document().dom, "to-about").unwrap();
        let outcome = browser.click_node(&link);
        assert!(matches!(outcome, ClickOutcome::Navigated(_)), "{outcome:?}");
        assert_eq!(browser.url().to_string(), "demo:///pages/about.html");
    }

    #[test]
    fn back_and_forward_walk_the_history() {
        let mut browser = browser();
        browser.follow_link("pages/about.html").unwrap();
        browser.follow_link("../index.html").unwrap();
        assert_eq!(browser.history().len(), 3);

        assert!(browser.back());
        assert_eq!(browser.url().to_string(), "demo:///pages/about.html");
        assert_eq!(browser.document().title().as_deref(), Some("About"));

        assert!(browser.back());
        assert_eq!(browser.url().to_string(), "demo:///index.html");
        assert!(!browser.can_go_back());
        assert!(!browser.back());

        assert!(browser.forward());
        assert_eq!(browser.url().to_string(), "demo:///pages/about.html");
        assert!(browser.forward());
        assert!(!browser.forward());
    }

    #[test]
    fn navigating_truncates_the_forward_history() {
        let mut browser = browser();
        browser.follow_link("pages/about.html").unwrap();
        browser.back();
        assert!(browser.can_go_forward());

        browser.follow_link("pages/about.html").unwrap();
        assert!(
            !browser.can_go_forward(),
            "a new navigation drops the forward entries"
        );
        assert_eq!(browser.history().len(), 2);
    }

    #[test]
    fn fragment_links_stay_on_the_same_document() {
        let mut browser = browser();
        let before = browser.document().dom.children.len();

        let link = dom_api::get_element_by_id(&browser.document().dom, "to-anchor").unwrap();
        assert!(matches!(
            browser.click_node(&link),
            ClickOutcome::Navigated(_)
        ));

        assert_eq!(browser.url().to_string(), "demo:///index.html#section");
        assert_eq!(browser.document().url.to_string(), "demo:///index.html");
        assert_eq!(browser.document().dom.children.len(), before);
        assert!(browser.can_go_back());
    }

    #[test]
    fn clicking_a_scripted_element_runs_the_handler_without_navigating() {
        let mut browser = browser();
        let button = dom_api::get_element_by_id(&browser.document().dom, "btn").unwrap();

        assert_eq!(browser.click_node(&button), ClickOutcome::Script);
        assert_eq!(browser.click_node(&button), ClickOutcome::Script);

        let text =
            dom_api::text_content(dom_api::node_at(&browser.document().dom, &button).unwrap());
        assert_eq!(
            text, "clicks: 2",
            "runtime state must survive between clicks"
        );
        assert_eq!(browser.history().len(), 1, "no navigation happened");
    }

    #[test]
    fn prevent_default_suppresses_link_navigation() {
        let mut loader = MemoryLoader::new();
        loader.insert(
            "demo:///index.html",
            r#"<a id="link" href="other.html">go</a>
               <script>
                 document.getElementById("link").addEventListener("click", function (e) {
                     e.preventDefault();
                 });
               </script>"#,
        );
        loader.insert("demo:///other.html", "<p>other</p>");

        let mut browser =
            Browser::open(Box::new(loader), &Url::parse("demo:///index.html").unwrap()).unwrap();
        let link = dom_api::get_element_by_id(&browser.document().dom, "link").unwrap();
        assert_eq!(browser.click_node(&link), ClickOutcome::Script);
        assert_eq!(browser.url().to_string(), "demo:///index.html");
    }

    #[test]
    fn a_broken_link_reports_the_failure_and_stays_put() {
        let mut loader = MemoryLoader::new();
        loader.insert(
            "demo:///index.html",
            r#"<a id="link" href="missing.html">go</a>"#,
        );
        let mut browser =
            Browser::open(Box::new(loader), &Url::parse("demo:///index.html").unwrap()).unwrap();

        let link = dom_api::get_element_by_id(&browser.document().dom, "link").unwrap();
        let outcome = browser.click_node(&link);
        assert!(
            matches!(outcome, ClickOutcome::NavigationFailed { .. }),
            "{outcome:?}"
        );
        assert_eq!(browser.url().to_string(), "demo:///index.html");
        assert_eq!(browser.history().len(), 1);
    }

    #[test]
    fn reload_rebuilds_the_document() {
        let mut browser = browser();
        let button = dom_api::get_element_by_id(&browser.document().dom, "btn").unwrap();
        browser.click_node(&button);

        browser.reload().expect("reloads");
        let text =
            dom_api::text_content(dom_api::node_at(&browser.document().dom, &button).unwrap());
        assert_eq!(text, "count", "reload starts the page over");
        assert_eq!(browser.history().len(), 1, "reload does not add history");
    }

    #[test]
    fn clicking_empty_space_is_ignored() {
        let mut browser = browser();
        assert_eq!(
            browser.click_at(10.0, 10_000.0, 800.0),
            ClickOutcome::Ignored
        );
    }

    #[test]
    fn status_line_shows_title_url_and_position() {
        let mut browser = browser();
        browser.follow_link("pages/about.html").unwrap();
        let status = browser.status_line();
        assert!(status.contains("About"), "{status}");
        assert!(status.contains("demo:///pages/about.html"), "{status}");
        assert!(status.contains("[2/2]"), "{status}");
    }

    // ── Forms and keyboard ────────────────────────────────────────────────

    fn form_site() -> MemoryLoader {
        let mut loader = MemoryLoader::new();
        loader.insert(
            "demo:///index.html",
            r#"<title>Search</title>
               <form id="f" action="results.html" method="get">
                 <input id="q" name="q" value="">
                 <input type="checkbox" name="exact" value="1" id="exact">
                 <input name="off" value="skip" disabled>
                 <button id="go" type="submit">Go</button>
               </form>"#,
        );
        loader.insert(
            "demo:///results.html",
            r#"<title>Results</title><p>results</p>"#,
        );
        loader
    }

    fn form_browser() -> Browser {
        Browser::open(
            Box::new(form_site()),
            &Url::parse("demo:///index.html").unwrap(),
        )
        .expect("session opens")
    }

    #[test]
    fn typing_and_submitting_navigates_with_a_query_string() {
        let mut browser = form_browser();
        let field = dom_api::get_element_by_id(&browser.document().dom, "q").unwrap();
        browser.document_mut().focus_path(&field);
        browser.type_text("toy browser");

        let button = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();
        let outcome = browser.click_node(&button);

        assert!(matches!(outcome, ClickOutcome::Navigated(_)), "{outcome:?}");
        assert_eq!(
            browser.url().to_string(),
            "demo:///results.html?q=toy+browser"
        );
        assert_eq!(browser.document().title().as_deref(), Some("Results"));
        assert_eq!(browser.history().len(), 2, "submission is a navigation");
        assert!(browser.can_go_back());
    }

    #[test]
    fn checked_boxes_join_the_query_and_unchecked_ones_do_not() {
        let mut browser = form_browser();
        let field = dom_api::get_element_by_id(&browser.document().dom, "q").unwrap();
        browser.document_mut().focus_path(&field);
        browser.type_text("a");

        let checkbox = dom_api::get_element_by_id(&browser.document().dom, "exact").unwrap();
        browser.click_node(&checkbox);

        let button = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();
        browser.click_node(&button);
        assert_eq!(
            browser.url().to_string(),
            "demo:///results.html?q=a&exact=1"
        );
    }

    #[test]
    fn enter_in_the_field_submits_through_the_browser() {
        let mut browser = form_browser();
        let field = dom_api::get_element_by_id(&browser.document().dom, "q").unwrap();
        browser.document_mut().focus_path(&field);
        browser.type_text("hi");

        let outcome = browser.press_key(&KeyEvent::new(Key::Enter));
        assert!(matches!(outcome, ClickOutcome::Navigated(_)), "{outcome:?}");
        assert_eq!(browser.url().to_string(), "demo:///results.html?q=hi");
    }

    #[test]
    fn going_back_after_a_submission_restores_the_form_page() {
        let mut browser = form_browser();
        let field = dom_api::get_element_by_id(&browser.document().dom, "q").unwrap();
        browser.document_mut().focus_path(&field);
        browser.type_text("x");
        browser.press_key(&KeyEvent::new(Key::Enter));

        assert!(browser.back());
        assert_eq!(browser.url().to_string(), "demo:///index.html");
        assert_eq!(browser.document().title().as_deref(), Some("Search"));
        // A fresh load, so the typed value is gone — there is no bfcache.
        let field = dom_api::get_element_by_id(&browser.document().dom, "q").unwrap();
        let element = dom_api::node_at(&browser.document().dom, &field)
            .unwrap()
            .as_element()
            .unwrap();
        assert_eq!(element.control_value(), "");
    }

    #[test]
    fn tab_moves_focus_through_the_browser() {
        let mut browser = form_browser();
        browser.press_key(&KeyEvent::new(Key::Tab));
        let focused = browser.document().focused_path().expect("focus");
        let element = dom_api::node_at(&browser.document().dom, &focused)
            .unwrap()
            .as_element()
            .unwrap();
        assert_eq!(element.get_attr("id"), Some("q"));
    }

    #[test]
    fn a_cancelled_submit_keeps_the_page() {
        let mut loader = MemoryLoader::new();
        loader.insert(
            "demo:///index.html",
            r#"<form id="f" action="next.html"><input name="q" value="v"><button id="go">Go</button></form>
               <script>
                 document.getElementById("f").addEventListener("submit", function (e) {
                     e.preventDefault();
                 });
               </script>"#,
        );
        loader.insert("demo:///next.html", "<p>next</p>");
        let mut browser =
            Browser::open(Box::new(loader), &Url::parse("demo:///index.html").unwrap()).unwrap();

        let button = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();
        browser.click_node(&button);
        assert_eq!(browser.url().to_string(), "demo:///index.html");
        assert_eq!(browser.history().len(), 1);
    }

    #[test]
    fn typing_then_rendering_shows_the_new_value() {
        let mut browser = form_browser();
        let before = browser
            .render(400, 120, 0.0, &PointerState::default())
            .to_ppm();

        let field = dom_api::get_element_by_id(&browser.document().dom, "q").unwrap();
        browser.document_mut().focus_path(&field);
        browser.type_text("visible");

        let after = browser
            .render(400, 120, 0.0, &PointerState::default())
            .to_ppm();
        assert_ne!(before, after, "typed text must reach the repaint");
    }

    #[test]
    fn post_forms_are_refused_rather_than_downgraded() {
        let mut loader = MemoryLoader::new();
        loader.insert(
            "demo:///index.html",
            r#"<form id="f" action="/p" method="post"><input name="q" value="v"><button id="go">Go</button></form>"#,
        );
        let mut browser =
            Browser::open(Box::new(loader), &Url::parse("demo:///index.html").unwrap()).unwrap();
        let button = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();
        let outcome = browser.click_node(&button);
        assert!(
            matches!(outcome, ClickOutcome::NavigationFailed { .. }),
            "{outcome:?}"
        );
        assert_eq!(browser.url().to_string(), "demo:///index.html");
    }

    // ── Event loop and navigation lifecycle ───────────────────────────────

    /// A session driven by virtual time, with the clock the test can advance.
    fn timed_browser(loader: MemoryLoader, url: &str) -> (Browser, Rc<ManualClock>) {
        let clock = Rc::new(ManualClock::new());
        let browser =
            Browser::open_with_clock(Box::new(loader), &Url::parse(url).unwrap(), clock.clone())
                .expect("session opens");
        (browser, clock)
    }

    fn ticking_site() -> MemoryLoader {
        let mut loader = MemoryLoader::new();
        loader.insert(
            "demo:///timers.html",
            r#"<title>Timers</title>
               <p id="status">waiting</p>
               <a id="away" href="other.html">leave</a>
               <script>
                 let ticks = 0;
                 setInterval(function () {
                     ticks = ticks + 1;
                     document.getElementById("status").textContent = "ticks " + ticks;
                 }, 100);
               </script>"#,
        );
        loader.insert(
            "demo:///other.html",
            r#"<title>Other</title><p id="status">other page</p>"#,
        );
        loader
    }

    fn status_of(browser: &Browser) -> String {
        let path =
            dom_api::query_selector(&browser.document().dom, &[], "#status").expect("#status");
        dom_api::text_content(dom_api::node_at(&browser.document().dom, &path).unwrap())
    }

    #[test]
    fn advancing_virtual_time_runs_the_page_timers() {
        let (mut browser, _clock) = timed_browser(ticking_site(), "demo:///timers.html");
        assert_eq!(status_of(&browser), "waiting");

        let report = browser.advance_time(Duration::from_millis(100));
        assert_eq!(report.timers_run, 1);
        assert_eq!(status_of(&browser), "ticks 1");

        browser.advance_time(Duration::from_millis(100));
        assert_eq!(status_of(&browser), "ticks 2");
    }

    #[test]
    fn ticking_without_advancing_time_does_nothing() {
        let (mut browser, _clock) = timed_browser(ticking_site(), "demo:///timers.html");
        let report = browser.tick();
        assert!(!report.did_work());
        assert_eq!(status_of(&browser), "waiting");
    }

    #[test]
    fn the_browser_reports_when_it_next_needs_a_turn() {
        let (mut browser, _clock) = timed_browser(ticking_site(), "demo:///timers.html");
        assert_eq!(browser.next_wakeup_in_ms(), Some(100.0));
        assert!(browser.has_pending_tasks());

        browser.advance_time(Duration::from_millis(60));
        assert_eq!(browser.next_wakeup_in_ms(), Some(40.0));
    }

    #[test]
    fn navigating_away_stops_the_old_page_timers() {
        let (mut browser, _clock) = timed_browser(ticking_site(), "demo:///timers.html");
        browser.advance_time(Duration::from_millis(100));
        assert_eq!(status_of(&browser), "ticks 1");

        let link = dom_api::get_element_by_id(&browser.document().dom, "away").unwrap();
        browser.click_node(&link);
        assert_eq!(browser.document().title().as_deref(), Some("Other"));

        // The new page has no tasks, and the old interval is gone with it.
        assert!(!browser.has_pending_tasks());
        let report =
            browser.advance_time_in_steps(Duration::from_millis(500), Duration::from_millis(50));
        assert_eq!(
            report.timers_run, 0,
            "the departed page must not keep running"
        );
        assert_eq!(status_of(&browser), "other page");
    }

    #[test]
    fn going_back_starts_the_page_over_with_a_fresh_scheduler() {
        let (mut browser, _clock) = timed_browser(ticking_site(), "demo:///timers.html");
        browser.advance_time_in_steps(Duration::from_millis(300), Duration::from_millis(100));
        assert_eq!(status_of(&browser), "ticks 3");

        let link = dom_api::get_element_by_id(&browser.document().dom, "away").unwrap();
        browser.click_node(&link);
        assert!(browser.back());

        // A fresh load: the counter restarts and its own interval runs again.
        assert_eq!(status_of(&browser), "waiting");
        browser.advance_time(Duration::from_millis(100));
        assert_eq!(status_of(&browser), "ticks 1");
    }

    #[test]
    fn reloading_restarts_the_timers() {
        let (mut browser, _clock) = timed_browser(ticking_site(), "demo:///timers.html");
        browser.advance_time_in_steps(Duration::from_millis(200), Duration::from_millis(100));
        assert_eq!(status_of(&browser), "ticks 2");

        browser.reload().expect("reloads");
        assert_eq!(status_of(&browser), "waiting");
        browser.advance_time(Duration::from_millis(100));
        assert_eq!(status_of(&browser), "ticks 1", "timing restarts from load");
    }

    #[test]
    fn a_click_can_schedule_work_that_lands_later() {
        let mut loader = MemoryLoader::new();
        loader.insert(
            "demo:///index.html",
            r#"<button id="go">go</button><p id="status">idle</p>
               <script>
                 document.getElementById("go").addEventListener("click", function () {
                     setTimeout(function () {
                         document.getElementById("status").textContent = "done";
                     }, 250);
                 });
               </script>"#,
        );
        let (mut browser, _clock) = timed_browser(loader, "demo:///index.html");

        let button = dom_api::get_element_by_id(&browser.document().dom, "go").unwrap();
        browser.click_node(&button);
        assert_eq!(status_of(&browser), "idle");

        browser.advance_time(Duration::from_millis(200));
        assert_eq!(status_of(&browser), "idle");
        browser.advance_time(Duration::from_millis(50));
        assert_eq!(status_of(&browser), "done");
    }

    #[test]
    fn animation_frames_run_and_repaint_between_ticks() {
        let mut loader = MemoryLoader::new();
        loader.insert(
            "demo:///anim.html",
            r#"<div id="box" style="width: 20px; height: 20px; background-color: rgb(10, 90, 200)"></div>
               <script>
                 let x = 0;
                 function step() {
                     x = x + 10;
                     document.getElementById("box").style.marginLeft = x + "px";
                     if (x < 50) { requestAnimationFrame(step); }
                 }
                 requestAnimationFrame(step);
               </script>"#,
        );
        let (mut browser, _clock) = timed_browser(loader, "demo:///anim.html");

        let mut frames = Vec::new();
        for _ in 0..3 {
            browser.advance_time(Duration::from_millis(16));
            frames.push(
                browser
                    .render(200, 60, 0.0, &PointerState::default())
                    .to_ppm(),
            );
        }
        assert_ne!(frames[0], frames[1], "frame 1 → 2 should differ");
        assert_ne!(frames[1], frames[2], "and moved again");
    }

    #[test]
    fn navigation_cancels_pending_animation_frames() {
        let mut loader = MemoryLoader::new();
        loader.insert(
            "demo:///anim.html",
            r#"<a id="away" href="other.html">leave</a>
               <script>
                 function step() { console.log("frame"); requestAnimationFrame(step); }
                 requestAnimationFrame(step);
               </script>"#,
        );
        loader.insert("demo:///other.html", "<p>other</p>");
        let (mut browser, _clock) = timed_browser(loader, "demo:///anim.html");

        browser.advance_time(Duration::from_millis(16));
        assert!(browser.has_pending_tasks());

        let link = dom_api::get_element_by_id(&browser.document().dom, "away").unwrap();
        browser.click_node(&link);
        assert!(
            !browser.has_pending_tasks(),
            "frame callbacks left with the page"
        );

        let report = browser.advance_time(Duration::from_millis(100));
        assert_eq!(report.frames_run, 0);
    }

    #[test]
    fn performance_now_follows_the_virtual_clock() {
        let mut loader = MemoryLoader::new();
        loader.insert(
            "demo:///time.html",
            r#"<p id="status">-</p>
               <script>
                 setInterval(function () {
                     document.getElementById("status").textContent = "" + performance.now();
                 }, 100);
               </script>"#,
        );
        let (mut browser, _clock) = timed_browser(loader, "demo:///time.html");

        browser.advance_time(Duration::from_millis(100));
        assert_eq!(status_of(&browser), "100");
        browser.advance_time(Duration::from_millis(150));
        assert_eq!(status_of(&browser), "250");
    }

    #[test]
    fn a_form_submitted_from_a_timer_navigates() {
        let mut loader = MemoryLoader::new();
        loader.insert(
            "demo:///index.html",
            r#"<form id="f" action="results.html"><input name="q" value="auto"></form>
               <script>
                 setTimeout(function () { document.getElementById("f").submit(); }, 50);
               </script>"#,
        );
        loader.insert("demo:///results.html", "<title>Results</title>");
        let (mut browser, _clock) = timed_browser(loader, "demo:///index.html");

        browser.advance_time(Duration::from_millis(50));
        assert_eq!(browser.url().to_string(), "demo:///results.html?q=auto");
        assert_eq!(browser.document().title().as_deref(), Some("Results"));
    }
}
