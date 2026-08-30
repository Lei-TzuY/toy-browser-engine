// ============================================================
//  browser_cookie_session_final.rs — navigation + cookie session
// ============================================================
//
//  This is the public Browser session facade on top of the current mainline
//  browser implementation. It keeps the same API while making one CookieJar
//  part of session state, just like history, localStorage and the network.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use crate::cookie_network::CookieJarRef;
use crate::cookie_same_site::SameSiteRequestContext;
use crate::document::{
    committed_referrer_context_from_response, Document, LoopReport, PageAction, PointerState,
};
use crate::document_referrer::DocumentReferrerContext;
use crate::eventloop::{Clock, RealClock};
use crate::forms::{self, encode_form_entries, Submission, SubmissionMethod};
use crate::input::{Key, KeyEvent};
use crate::navigation_network::NavigationNetwork;
use crate::net::{
    DefaultNetwork, FetchError, FetchRequest, HeaderMap, LoadError, Method, NetworkBackend,
    ResourceLoader, Url,
};
use crate::script::interp::{EventInit, JsValue, StorageRef};
use crate::script::NodePath;
use crate::session_network::SessionNetwork;
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

/// Navigation metadata owned by the activated `<a>` element.
///
/// Keeping this separate from the URL prevents element-only policy such as
/// `referrerpolicy` and `rel=noreferrer` from leaking into ordinary calls to
/// [`Browser::navigate`] or [`Browser::follow_link`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct HyperlinkActivation {
    url: Url,
    referrerpolicy: Option<String>,
    rel: Option<String>,
}

pub struct Browser {
    /// Shared with the network, which may hand it to a worker thread — hence
    /// `Arc` rather than `Box`. The public constructors still take a `Box`, so
    /// nothing outside has to know.
    loader: Arc<dyn ResourceLoader>,
    /// Where `fetch()` goes. One per session, shared by every document it
    /// shows. `SessionNetwork` applies HSTS before cookie selection so Secure
    /// cookies observe the URL that actually reaches transport.
    network: Rc<dyn NetworkBackend>,
    /// Synchronous top-level navigation policy sharing the exact CookieJar,
    /// HSTS cache and clock used by `network`.
    navigation: NavigationNetwork,
    /// The one cookie jar owned by this browsing session. Documents and Fetch
    /// receive the same Rc, so `document.cookie` and Set-Cookie converge.
    cookie_jar: CookieJarRef,
    /// Visited URLs, oldest first.
    history: Vec<Url>,
    /// Index into `history` of the entry on screen.
    index: usize,
    document: Document,
    /// Referrer source/policy frozen when `document` was committed. This is
    /// kept beside the current Document because the final HTTP response header
    /// is browsing-session state; standalone Documents may not have one.
    document_referrer: DocumentReferrerContext,
    /// Drives the event loop. Real time in a window, virtual time in tests.
    clock: Rc<dyn Clock>,
    /// Loop time at which the current document started, so `performance.now()`
    /// and frame timestamps are measured from page load.
    document_epoch_ms: f64,
    /// Origin-scoped persistent localStorage pools shared across navigation.
    pub local_storage_pool: Rc<RefCell<HashMap<String, StorageRef>>>,
}

impl Browser {
    /// Load `url` and start a session at it, driven by the real clock.
    pub fn open(loader: Box<dyn ResourceLoader>, url: &Url) -> Result<Browser, LoadError> {
        Browser::open_with_clock(loader, url, Rc::new(RealClock::new()))
    }

    /// Load `url` with a caller-supplied clock.
    ///
    /// Tests pass a [`ManualClock`](crate::eventloop::ManualClock) so time only
    /// moves when they say so, which is what makes timer and cookie-expiry
    /// tests deterministic without sleeping.
    pub fn open_with_clock(
        loader: Box<dyn ResourceLoader>,
        url: &Url,
        clock: Rc<dyn Clock>,
    ) -> Result<Browser, LoadError> {
        let loader: Arc<dyn ResourceLoader> = Arc::from(loader);
        let network = Rc::new(DefaultNetwork::new(loader.clone()));
        Browser::open_with(loader, network, url, clock)
    }

    /// Load `url` with a caller-supplied clock *and* transport backend.
    ///
    /// Browser policy wraps that transport in the canonical `SessionNetwork`.
    /// Tests can keep their own Rc to a `ManualNetwork` and inspect exactly
    /// what reached transport after HSTS and cookie policy have run.
    pub fn open_with_network(
        loader: Box<dyn ResourceLoader>,
        network: Rc<dyn NetworkBackend>,
        url: &Url,
        clock: Rc<dyn Clock>,
    ) -> Result<Browser, LoadError> {
        Browser::open_with(Arc::from(loader), network, url, clock)
    }

    fn storage_for_url(&self, url: &Url) -> StorageRef {
        let origin = format!("{}://{}", url.scheme(), url.host());
        self.local_storage_pool
            .borrow_mut()
            .entry(origin)
            .or_insert_with(|| Rc::new(RefCell::new(Vec::new())))
            .clone()
    }

    fn open_with(
        loader: Arc<dyn ResourceLoader>,
        transport: Rc<dyn NetworkBackend>,
        url: &Url,
        clock: Rc<dyn Clock>,
    ) -> Result<Browser, LoadError> {
        let pool: Rc<RefCell<HashMap<String, StorageRef>>> =
            Rc::new(RefCell::new(HashMap::new()));

        // Session policy must exist before the first HTTP response arrives so
        // Set-Cookie/HSTS are visible to bootstrap scripts on the first page.
        let session_network = SessionNetwork::with_new_state(transport, clock.clone());
        let cookie_jar = session_network.cookie_jar();
        let hsts_cache = session_network.hsts_cache();
        let navigation = NavigationNetwork::new(
            loader.clone(),
            cookie_jar.clone(),
            hsts_cache,
            clock.clone(),
        );
        let initial = navigation.load_initial(url)?;
        let document_referrer = committed_referrer_context_from_response(&initial);
        let final_url = initial.url.clone();
        let origin = format!("{}://{}", final_url.scheme(), final_url.host());
        let storage = pool
            .borrow_mut()
            .entry(origin)
            .or_insert_with(|| Rc::new(RefCell::new(Vec::new())))
            .clone();

        let network: Rc<dyn NetworkBackend> = Rc::new(session_network);
        let mut document = Document::from_response_with_session_subresources(
            &initial,
            &navigation,
            Some(storage),
            Some(cookie_jar.clone()),
        );
        document.set_network(network.clone());
        let epoch = clock.now_ms();
        Ok(Browser {
            loader,
            network,
            navigation,
            cookie_jar,
            history: vec![document.url.clone()],
            index: 0,
            document,
            document_referrer,
            clock,
            document_epoch_ms: epoch,
            local_storage_pool: pool,
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

    /// Shared cookie storage for this browser session.
    pub fn cookie_jar(&self) -> CookieJarRef {
        self.cookie_jar.clone()
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

    /// Fetch and construct one top-level HTML document through session policy.
    fn load_navigation_document(
        &self,
        url: &Url,
        method: Method,
    ) -> Result<(Document, Url, DocumentReferrerContext), LoadError> {
        // HSTS runs before cookie selection. SameSite is schemeful, so classify
        // the target that will actually be dispatched rather than the authored
        // pre-upgrade HTTP spelling.
        let effective_target = self.navigation.effective_url(url);
        let context = top_level_context(&self.document.url, &effective_target, method);
        let response = self
            .navigation
            .get(url, context)
            .map_err(|error| load_error_from_fetch(url, error))?;
        if !response.ok() {
            return Err(LoadError::HttpStatus {
                url: response.url.to_string(),
                status: response.status,
            });
        }

        let document_referrer = committed_referrer_context_from_response(&response);
        let final_url = response.url.clone();
        let storage = self.storage_for_url(&final_url);
        let document = Document::from_response_with_session_subresources(
            &response,
            &self.navigation,
            Some(storage),
            Some(self.cookie_jar.clone()),
        );
        Ok((document, final_url, document_referrer))
    }

    /// Fetch and construct one document from an activated HTML hyperlink.
    ///
    /// Unlike ordinary programmatic navigation, this path carries the
    /// activated element's referrer controls into the first request and keeps
    /// the resulting redirect state alive across every hop. The committed
    /// document still derives its own fresh policy from the final response and
    /// parser-time metadata.
    fn load_hyperlink_document(
        &self,
        activation: &HyperlinkActivation,
    ) -> Result<(Document, Url, DocumentReferrerContext), LoadError> {
        let effective_target = self.navigation.effective_url(&activation.url);
        let context = top_level_context(&self.document.url, &effective_target, Method::Get);
        let request = FetchRequest::get(activation.url.clone());
        let (response, _) = self
            .document_referrer
            .fetch_hyperlink_navigation(
                &self.navigation,
                &request,
                context,
                activation.referrerpolicy.as_deref(),
                activation.rel.as_deref(),
            )
            .map_err(|error| load_error_from_fetch(&activation.url, error))?;
        if !response.ok() {
            return Err(LoadError::HttpStatus {
                url: response.url.to_string(),
                status: response.status,
            });
        }

        let document_referrer = committed_referrer_context_from_response(&response);
        let final_url = response.url.clone();
        let storage = self.storage_for_url(&final_url);
        let document = Document::from_response_with_session_subresources(
            &response,
            &self.navigation,
            Some(storage),
            Some(self.cookie_jar.clone()),
        );
        Ok((document, final_url, document_referrer))
    }

    /// Navigate to `url`, pushing a history entry.
    ///
    /// A URL that differs only by fragment does not refetch the document, the
    /// same way a real browser jumps within the current page.
    pub fn navigate(&mut self, url: &Url) -> Result<(), LoadError> {
        let history_url = if !url.same_document(self.url()) {
            let (document, final_url, document_referrer) =
                self.load_navigation_document(url, Method::Get)?;
            self.replace_document(document, document_referrer);
            final_url
        } else {
            url.clone()
        };
        self.history.truncate(self.index + 1);
        self.history.push(history_url);
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

    /// Snapshot the live `<a>` default-action state after click listeners ran.
    ///
    /// This deliberately mirrors `Document::link_at`'s ancestor walk, but also
    /// retains the element-only policy inputs Browser needs to dispatch the
    /// navigation correctly. Reading after event dispatch means authored code
    /// may still change `href`, `referrerpolicy`, or `rel` before the default
    /// action is selected.
    fn hyperlink_at(&self, path: &[usize]) -> Option<HyperlinkActivation> {
        for ancestor in crate::script::dom_api::ancestor_paths(path) {
            let node = crate::script::dom_api::node_at(&self.document.dom, &ancestor)?;
            let Some(element) = node.as_element() else {
                continue;
            };
            if element.tag_name != "a" {
                continue;
            }
            let href = element.get_attr("href")?;
            let url = self.document.resolve(href)?;
            return Some(HyperlinkActivation {
                url,
                referrerpolicy: element.get_attr("referrerpolicy").map(|value| value.to_string()),
                rel: element.get_attr("rel").map(|value| value.to_string()),
            });
        }
        None
    }

    /// Perform one hyperlink default action while preserving ordinary history
    /// and same-document fragment semantics.
    fn navigate_hyperlink(&mut self, activation: &HyperlinkActivation) -> Result<(), LoadError> {
        let history_url = if !activation.url.same_document(self.url()) {
            let (document, final_url, document_referrer) =
                self.load_hyperlink_document(activation)?;
            self.replace_document(document, document_referrer);
            final_url
        } else {
            activation.url.clone()
        };
        self.history.truncate(self.index + 1);
        self.history.push(history_url);
        self.index = self.history.len() - 1;
        Ok(())
    }

    /// Reload the current entry from its source.
    pub fn reload(&mut self) -> Result<(), LoadError> {
        let url = self.url().clone();
        let (document, _final_url, document_referrer) =
            self.load_navigation_document(&url, Method::Get)?;
        self.replace_document(document, document_referrer);
        Ok(())
    }

    /// Put a freshly loaded document and its committed policy on screen.
    ///
    /// The outgoing document — and with it every timer, interval and
    /// animation-frame callback its scripts registered — is dropped here, so a
    /// page that has been navigated away from cannot keep running.
    fn replace_document(
        &mut self,
        mut document: Document,
        document_referrer: DocumentReferrerContext,
    ) {
        // Cancelling first drops the outgoing page's fetch registry, and with
        // it every pending promise: an answer that arrives afterwards has
        // nothing left to settle and is discarded.
        self.document.cancel_all_tasks();
        // Internal navigation paths inject the jar before bootstrap scripts.
        // Reassert the invariant here as well for any future replacement path
        // that hands Browser an already-built Document.
        document.runtime.cookie_jar = self.cookie_jar.clone();
        document.set_network(self.network.clone());
        self.document = document;
        self.document_referrer = document_referrer;
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
        match self.load_navigation_document(&url, Method::Get) {
            Ok((document, _final_url, document_referrer)) => {
                self.replace_document(document, document_referrer)
            }
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
        // Handlers may have added images or rewritten the page. Keep those
        // loads above the same CORS/credential boundary as parser-time images,
        // using the exact policy frozen when this document was committed.
        if outcome.dispatched {
            let referrer = self.document_referrer.clone();
            self.document
                .refresh_images_with_committed_referrer(&self.navigation, &referrer);
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
        match self.hyperlink_at(path) {
            Some(activation) => {
                let url = activation.url.clone();
                match self.navigate_hyperlink(&activation) {
                    Ok(()) => ClickOutcome::Navigated(url),
                    Err(error) => ClickOutcome::NavigationFailed { url, error },
                }
            }
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

    /// The browser-policy network this session fetches through.
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
        let effective_target = self.navigation.effective_url(&submission.url);
        let context = top_level_context(&self.document.url, &effective_target, Method::Post);

        match self.navigation.fetch(&request, context) {
            Ok(response) if response.ok() => {
                let document_referrer = committed_referrer_context_from_response(&response);
                let final_url = response.url.clone();
                let storage = self.storage_for_url(&final_url);
                let document = Document::from_response_with_session_subresources(
                    &response,
                    &self.navigation,
                    Some(storage),
                    Some(self.cookie_jar.clone()),
                );
                self.replace_document(document, document_referrer);
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

/// Conservative schemeful-site approximation used until the URL/security layer
/// owns a Public Suffix List. Exact scheme+host equality is never more
/// permissive than real schemeful same-site and therefore cannot expose a
/// Strict/Lax cookie to a broader site than intended. Subdomains that would be
/// same-site under registrable-domain rules are deliberately treated as
/// cross-site for now.
fn conservative_same_site(source: &Url, target: &Url) -> bool {
    source.scheme() == target.scheme() && source.host().eq_ignore_ascii_case(target.host())
}

fn top_level_context(source: &Url, target: &Url, method: Method) -> SameSiteRequestContext {
    SameSiteRequestContext::new(conservative_same_site(source, target), true, method)
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