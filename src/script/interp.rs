// ============================================================
//  script/interp.rs  —  Tree-walking interpreter & DOM bindings
// ============================================================
//
//  `JsRuntime` owns the global scope, the registered event
//  listeners, and the pool of detached nodes created by
//  `document.createElement`.  The DOM itself is *not* owned: it is
//  passed in as `&mut Node` on every entry point, so one runtime can
//  outlive many script runs and event dispatches — which is what
//  makes state (a click counter, say) persist between events.
//
//  Element handles are paths into the document tree (see `dom_api`).
//  Nodes created but not yet inserted live in `detached`; when they
//  are appended, the slot records where they went so handles taken
//  before insertion keep working.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use crate::dom::{ElementId, Node, NodeType};
use crate::eventloop::{MicrotaskQueue, Scheduler};
use crate::net::fetch::FetchRegistry;

use super::ast::*;
use super::dom_api::{self, NodePath};
use super::host::HostObject;
use super::parser::Parser;
use super::promise::{self, Microtask, PromiseRef, ReactionKind};

pub use super::fetch_api::PendingFetch;

/// Maximum nested function calls before a script is abandoned.
const MAX_CALL_DEPTH: usize = 128;
/// Maximum iterations of a single loop before it is abandoned.
const MAX_LOOP_ITERATIONS: usize = 200_000;

// ── Node handles ──────────────────────────────────────────────────────────────

/// A reference to a DOM node, either live in the document or inside a
/// detached subtree waiting to be inserted.
#[derive(Debug, Clone, PartialEq)]
pub enum NodeRef {
    Tree(NodePath),
    Detached { slot: usize, path: NodePath },
}

#[derive(Debug, Clone, PartialEq)]
enum Resolved {
    InTree(NodePath),
    InPool { slot: usize, path: NodePath },
    Gone,
}

#[derive(Debug)]
struct DetachedSlot {
    node: Option<Node>,
    /// Where this subtree's root moved to once it was appended somewhere.
    alias: Option<NodeRef>,
}

// ── Values ────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub enum JsValue {
    Undefined,
    Null,
    Bool(bool),
    Number(f32),
    Str(String),
    Array(Rc<RefCell<Vec<JsValue>>>),
    Object(Rc<RefCell<Vec<(String, JsValue)>>>),
    Function(Rc<FunctionValue>),
    /// A DOM element handle.
    Element(NodeRef),
    /// `element.style`
    Style(NodeRef),
    /// `element.classList`
    ClassList(NodeRef),
    /// `element.dataset`
    Dataset(NodeRef),
    /// `window.getComputedStyle(element)`
    ComputedStyle(NodeRef),
    /// A built-in namespace or function (`document`, `console`, `Math`, `parseInt`).
    Builtin(Builtin),
    /// A promise, shared with everything that holds a handle to it.
    Promise(PromiseRef),
    /// The `resolve`/`reject` function handed to a `new Promise` executor.
    PromiseResolver {
        promise: PromiseRef,
        reject: bool,
    },
    /// A native handler bound to one entry of `Promise.all` and friends.
    Combinator(Rc<CombinatorHandler>),
    /// A Web-platform object — `Headers`, `Request`, `Response`, and the
    /// abort pair. One arm covers all of them, so adding an API to the
    /// platform does not widen this enum.
    Host(Rc<HostObject>),
}

/// Which `Promise.*` combinator a native handler belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CombinatorKind {
    All,
    Race,
    AllSettled,
    Any,
}

/// State shared by the handlers of a single `Promise.all`-style call.
#[derive(Debug)]
pub struct CombinatorHandler {
    kind: CombinatorKind,
    /// Position of this entry in the input, so results keep their order.
    index: usize,
    /// True for the rejection handler of this entry.
    rejected: bool,
    slots: Rc<RefCell<Vec<JsValue>>>,
    remaining: Rc<RefCell<usize>>,
    result: PromiseRef,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Builtin {
    Window,
    Document,
    Console,
    EventCtor,
    CustomEventCtor,
    DOMParserCtor,
    DOMParser,
    XMLSerializerCtor,
    XMLSerializer,
    GetComputedStyle,
    PromiseCtor,
    QueueMicrotask,
    Fetch,
    HeadersCtor,
    RequestCtor,
    ResponseCtor,
    AbortControllerCtor,
    URLCtor,
    URLSearchParamsCtor,
    AudioContextCtor,
    LocalStorage,
    SessionStorage,
    StorageCtor,
    SetTimeout,
    ClearTimeout,
    SetInterval,
    ClearInterval,
    RequestAnimationFrame,
    CancelAnimationFrame,
    RequestIdleCallback,
    CancelIdleCallback,
    StructuredClone,
    DateMeta,
    Math,
    Json,
    Performance,
    ObjectMeta,
    EncodeUriComponent,
    DecodeUriComponent,
    Btoa,
    Atob,
    Location,
    Navigator,
    Screen,
    History,
    ParseInt,
    ParseFloat,
    StringConv,
    NumberConv,
    BooleanConv,
    IsNaN,
    IntersectionObserverCtor,
    MapCtor,
    SetCtor,
    Crypto,
}

pub struct FunctionValue {
    pub params: Vec<String>,
    pub body: Vec<Stmt>,
    /// Defining scope — this is what makes closures work.
    pub scope: ScopeRef,
}

impl fmt::Debug for FunctionValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[Function({})]", self.params.join(", "))
    }
}

impl fmt::Debug for JsValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JsValue::Undefined => write!(f, "undefined"),
            JsValue::Null => write!(f, "null"),
            JsValue::Bool(b) => write!(f, "{}", b),
            JsValue::Number(n) => write!(f, "{}", number_to_string(*n)),
            JsValue::Str(s) => write!(f, "{:?}", s),
            JsValue::Array(items) => write!(f, "{:?}", items.borrow()),
            JsValue::Object(props) => write!(f, "{:?}", props.borrow()),
            JsValue::Function(func) => write!(f, "{:?}", func),
            JsValue::Element(r) => write!(f, "[Element {:?}]", r),
            JsValue::Style(_) => write!(f, "[CSSStyleDeclaration]"),
            JsValue::ClassList(_) => write!(f, "[DOMTokenList]"),
            JsValue::Dataset(_) => write!(f, "[DOMStringMap]"),
            JsValue::ComputedStyle(_) => write!(f, "[CSSStyleDeclaration]"),
            JsValue::Builtin(b) => write!(f, "[{:?}]", b),
            JsValue::Promise(state) => write!(f, "[Promise {:?}]", state.borrow().status),
            JsValue::PromiseResolver { reject, .. } => {
                write!(f, "[PromiseResolver reject={reject}]")
            }
            JsValue::Combinator(handler) => write!(f, "[{:?} handler]", handler.kind),
            JsValue::Host(host) => write!(f, "[{}]", host.type_name()),
        }
    }
}

// ── Scopes ────────────────────────────────────────────────────────────────────

pub type ScopeRef = Rc<RefCell<Scope>>;

pub struct Scope {
    vars: HashMap<String, JsValue>,
    parent: Option<ScopeRef>,
}

impl Scope {
    fn new(parent: Option<ScopeRef>) -> ScopeRef {
        Rc::new(RefCell::new(Scope {
            vars: HashMap::new(),
            parent,
        }))
    }
}

fn scope_lookup(scope: &ScopeRef, name: &str) -> Option<JsValue> {
    let s = scope.borrow();
    if let Some(v) = s.vars.get(name) {
        return Some(v.clone());
    }
    let parent = s.parent.clone();
    drop(s);
    parent.and_then(|p| scope_lookup(&p, name))
}

/// Assign to an existing binding; returns false if the name is undeclared.
fn scope_assign(scope: &ScopeRef, name: &str, value: JsValue) -> bool {
    let mut s = scope.borrow_mut();
    if let Some(slot) = s.vars.get_mut(name) {
        *slot = value;
        return true;
    }
    let parent = s.parent.clone();
    drop(s);
    match parent {
        Some(p) => scope_assign(&p, name, value),
        None => false,
    }
}

fn scope_declare(scope: &ScopeRef, name: &str, value: JsValue) {
    scope.borrow_mut().vars.insert(name.to_string(), value);
}

// ── Control flow ──────────────────────────────────────────────────────────────

enum Flow {
    Normal,
    Return(JsValue),
    Break,
    Continue,
    /// An exception is propagating; the value itself is parked on the runtime
    /// so that expression evaluation — which cannot return a `Flow` — can
    /// signal a throw too.
    Throw,
}

// ── Listeners ─────────────────────────────────────────────────────────────────

/// What a dispatched event did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct EventOutcome {
    /// At least one listener ran.
    pub dispatched: bool,
    /// A listener called `preventDefault()`, so the browser's default action
    /// (following a link, say) must not happen.
    pub default_prevented: bool,
}

/// How an event should be dispatched.
#[derive(Debug, Clone, Default)]
pub struct EventInit {
    /// Whether the event travels up the ancestor chain. `focus` and `blur` do
    /// not; `focusin`, `focusout`, `input`, `change` and `submit` do.
    pub bubbles: bool,
    /// Extra properties placed on the event object.
    pub fields: Vec<(String, JsValue)>,
}

impl EventInit {
    pub fn bubbling() -> EventInit {
        EventInit {
            bubbles: true,
            ..EventInit::default()
        }
    }

    pub fn non_bubbling() -> EventInit {
        EventInit::default()
    }

    pub fn with_field(mut self, name: &str, value: JsValue) -> EventInit {
        self.fields.push((name.to_string(), value));
        self
    }
}

/// Something a script asked the browser to do that the interpreter cannot do
/// on its own, because it needs document- or session-level state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingAction {
    Focus(ElementId),
    Blur(ElementId),
    /// `form.submit()` — submits without firing a `submit` event, as in the DOM.
    Submit(ElementId),
    Reset(ElementId),
    Reload,
    Back,
    Forward,
}

#[derive(Debug, Clone)]
pub struct Listener {
    pub id: usize,
    pub target: NodeRef,
    pub event: String,
    pub handler: JsValue,
    pub capture: bool,
    pub once: bool,
    pub passive: bool,
}

pub type StorageRef = Rc<RefCell<Vec<(String, String)>>>;

// ── Runtime ───────────────────────────────────────────────────────────────────

pub struct JsRuntime {
    pub global: ScopeRef,
    pub listeners: Vec<Listener>,
    next_listener_id: usize,
    /// Persistent storage across the origin.
    pub local_storage: StorageRef,
    /// Ephemeral storage for this session/document.
    pub session_storage: StorageRef,
    /// Origin and path scoped cookie storage.
    pub cookie_jar: Rc<RefCell<crate::cookie::CookieJar>>,
    /// Everything scripts printed, newest last. Also echoed to stdout.
    pub console: Vec<String>,
    /// When true, console output is only recorded, not printed (used by tests).
    pub quiet: bool,
    /// Mirror of the document's focused element, so `document.activeElement`
    /// and `:focus` agree without the interpreter owning focus itself.
    pub focused: Option<ElementId>,
    /// Actions scripts requested; the document drains these after each run.
    pub pending: Vec<PendingAction>,
    /// The exception currently propagating, if any. Evaluation checks this at
    /// every junction and unwinds until a `try` catches it or the call
    /// finishes.
    exception: Option<JsValue>,
    pub url: crate::net::url::Url,
    /// Promise reactions and `queueMicrotask` callbacks, drained to empty at
    /// every checkpoint.
    pub microtasks: MicrotaskQueue<Microtask>,
    /// Timers and animation-frame requests made by this page's scripts.
    ///
    /// The runtime stores them (so `setTimeout` can hand back an id straight
    /// away) but never decides when they run: the document drives the loop.
    pub scheduler: Scheduler<JsValue>,
    /// Event-loop time, injected before each script or callback runs. The
    /// interpreter never reads a system clock of its own.
    pub now_ms: f64,
    /// Requests this page has in flight.
    ///
    /// Like the scheduler, the runtime only *records* them: `fetch()` hands
    /// back a pending promise and queues the request, and the document is what
    /// hands it to the network and brings the answer back.
    pub fetches: FetchRegistry<PendingFetch>,
    detached: Vec<DetachedSlot>,
    depth: usize,
}

impl Default for JsRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl JsRuntime {
    pub fn new() -> Self {
        Self {
            global: Scope::new(None),
            listeners: Vec::new(),
            next_listener_id: 1,
            local_storage: Rc::new(RefCell::new(Vec::new())),
            session_storage: Rc::new(RefCell::new(Vec::new())),
            cookie_jar: Rc::new(RefCell::new(crate::cookie::CookieJar::new())),
            console: Vec::new(),
            quiet: false,
            focused: None,
            pending: Vec::new(),
            exception: None,
            microtasks: MicrotaskQueue::new(),
            scheduler: Scheduler::new(),
            now_ms: 0.0,
            fetches: FetchRegistry::new(),
            url: crate::net::url::Url::parse("demo:///index.html").unwrap(),
            detached: Vec::new(),
            depth: 0,
        }
    }

    // ── Exceptions ────────────────────────────────────────────────────────

    /// Start an exception propagating.
    pub fn throw_value(&mut self, value: JsValue) {
        self.exception = Some(value);
    }

    /// Throw a `TypeError`-style string, the engine's own error shape.
    pub(crate) fn throw_type_error(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.throw_value(JsValue::Str(format!("TypeError: {message}")));
    }

    /// True while an exception is unwinding.
    pub fn has_exception(&self) -> bool {
        self.exception.is_some()
    }

    /// Take the propagating exception, stopping the unwind.
    pub fn take_exception(&mut self) -> Option<JsValue> {
        self.exception.take()
    }

    /// Call a callback, reporting anything it throws to the console.
    ///
    /// Every callback boundary — a timer, a frame, an event listener — ends
    /// here, so an exception is contained to the callback that raised it and
    /// never leaks into the next one.
    pub fn call_reporting(
        &mut self,
        dom: &mut Node,
        callee: &JsValue,
        args: Vec<JsValue>,
        context: &str,
    ) -> JsValue {
        match self.call_catching(dom, callee, args) {
            Ok(value) => value,
            Err(error) => {
                let text = to_string(&error);
                self.log(format!("Uncaught (in {context}) {text}"));
                JsValue::Undefined
            }
        }
    }

    /// Call a function and catch anything it throws.
    ///
    /// This is what turns a JavaScript exception into a rejected promise.
    pub fn call_catching(
        &mut self,
        dom: &mut Node,
        callee: &JsValue,
        args: Vec<JsValue>,
    ) -> Result<JsValue, JsValue> {
        let value = self.call_value(dom, callee, args);
        match self.take_exception() {
            Some(error) => Err(error),
            None => Ok(value),
        }
    }

    /// Report an exception that nothing caught, the way a browser console does.
    fn report_uncaught(&mut self) {
        if let Some(error) = self.take_exception() {
            let text = to_string(&error);
            self.log(format!("Uncaught {text}"));
        }
    }

    // ── Microtasks ────────────────────────────────────────────────────────

    /// Queue a microtask to run at the next checkpoint.
    pub fn queue_microtask(&mut self, task: Microtask) {
        self.microtasks.enqueue(task);
    }

    fn queue_microtasks(&mut self, tasks: Vec<Microtask>) {
        for task in tasks {
            self.microtasks.enqueue(task);
        }
    }

    /// Run every queued microtask, including ones queued while draining.
    ///
    /// This is the *microtask checkpoint*. It runs to exhaustion, so a promise
    /// chain settles completely before the loop moves on; the budget only
    /// exists to stop a callback that re-queues itself forever from wedging
    /// the browser, and anything left over runs at the next checkpoint.
    pub fn drain_microtasks(&mut self, dom: &mut Node) -> usize {
        let budget = self.microtasks.budget();
        let mut ran = 0;
        while let Some(task) = self.microtasks.pop() {
            if ran >= budget {
                self.log(
                    "microtask checkpoint budget exhausted; the rest run at the next checkpoint",
                );
                self.microtasks.enqueue(task);
                break;
            }
            ran += 1;
            self.run_microtask(dom, task);
        }
        ran
    }

    fn run_microtask(&mut self, dom: &mut Node, task: Microtask) {
        match task {
            Microtask::Callback(callback) => {
                // A failing microtask is reported; the rest still run.
                if let Err(error) = self.call_catching(dom, &callback, Vec::new()) {
                    let text = to_string(&error);
                    self.log(format!("Uncaught (in microtask) {text}"));
                }
            }
            Microtask::Reaction {
                reaction,
                value,
                rejected,
            } => self.run_reaction(dom, reaction, value, rejected),
        }
    }

    /// Run one promise reaction and settle the promise that `then` returned.
    fn run_reaction(
        &mut self,
        dom: &mut Node,
        reaction: promise::Reaction,
        value: JsValue,
        rejected: bool,
    ) {
        let handler = if rejected {
            reaction.on_rejected.clone()
        } else {
            reaction.on_fulfilled.clone()
        };

        let settle = match reaction.kind {
            ReactionKind::Then => match handler {
                Some(handler) => match self.call_catching(dom, &handler, vec![value]) {
                    // The returned value resolves the child, which is where
                    // `return anotherPromise` gets adopted.
                    Ok(result) => promise::resolve(&reaction.child, result),
                    Err(error) => promise::reject(&reaction.child, error),
                },
                // No handler for this outcome: pass it straight through.
                None if rejected => promise::reject(&reaction.child, value),
                None => promise::resolve(&reaction.child, value),
            },
            ReactionKind::Finally => {
                let outcome = match handler {
                    Some(handler) => self.call_catching(dom, &handler, Vec::new()),
                    None => Ok(JsValue::Undefined),
                };
                match outcome {
                    // The original outcome survives a successful cleanup…
                    Ok(_) if rejected => promise::reject(&reaction.child, value),
                    Ok(_) => promise::resolve(&reaction.child, value),
                    // …but a throwing cleanup replaces it.
                    Err(error) => promise::reject(&reaction.child, error),
                }
            }
        };
        self.queue_microtasks(settle);
    }

    // ── Promises ──────────────────────────────────────────────────────────

    /// Resolve a promise and queue whatever reactions that released.
    pub(crate) fn settle_resolve(&mut self, target: &PromiseRef, value: JsValue) {
        let tasks = promise::resolve(target, value);
        self.queue_microtasks(tasks);
    }

    /// Reject a promise and queue whatever reactions that released.
    pub(crate) fn settle_reject(&mut self, target: &PromiseRef, reason: JsValue) {
        let tasks = promise::reject(target, reason);
        self.queue_microtasks(tasks);
    }

    /// `new Promise(executor)`, and any other constructible builtin.
    fn construct(&mut self, dom: &mut Node, constructor: &JsValue, args: Vec<JsValue>) -> JsValue {
        match constructor {
            JsValue::Builtin(Builtin::PromiseCtor) => {
                let target = promise::new_promise();
                let Some(executor) = args.first().filter(|value| is_callable(value)).cloned()
                else {
                    self.throw_type_error("Promise resolver is not a function");
                    return JsValue::Undefined;
                };

                // The executor runs synchronously, and a throw inside it
                // rejects the promise rather than escaping.
                let resolve = JsValue::PromiseResolver {
                    promise: target.clone(),
                    reject: false,
                };
                let reject = JsValue::PromiseResolver {
                    promise: target.clone(),
                    reject: true,
                };
                if let Err(error) = self.call_catching(dom, &executor, vec![resolve, reject]) {
                    self.settle_reject(&target, error);
                }
                JsValue::Promise(target)
            }
            JsValue::Builtin(Builtin::EventCtor | Builtin::CustomEventCtor) => {
                let type_name = args.first().map(to_string).unwrap_or_default();
                let options = args.get(1);
                let (bubbles, cancelable, detail) = if let Some(JsValue::Object(props)) = options {
                    let b = object_get(props, "bubbles").map(|v| to_boolean(&v)).unwrap_or(false);
                    let c = object_get(props, "cancelable").map(|v| to_boolean(&v)).unwrap_or(false);
                    let d = object_get(props, "detail").unwrap_or(JsValue::Null);
                    (b, c, d)
                } else {
                    (false, false, JsValue::Null)
                };

                let mut fields = vec![
                    ("type".to_string(), JsValue::Str(type_name)),
                    ("bubbles".to_string(), JsValue::Bool(bubbles)),
                    ("cancelable".to_string(), JsValue::Bool(cancelable)),
                    ("defaultPrevented".to_string(), JsValue::Bool(false)),
                    ("cancelBubble".to_string(), JsValue::Bool(false)),
                    ("stopImmediate".to_string(), JsValue::Bool(false)),
                    ("eventPhase".to_string(), JsValue::Number(0.0)),
                    ("target".to_string(), JsValue::Null),
                    ("currentTarget".to_string(), JsValue::Null),
                ];
                if matches!(constructor, JsValue::Builtin(Builtin::CustomEventCtor)) {
                    fields.push(("detail".to_string(), detail));
                }
                JsValue::Object(Rc::new(RefCell::new(fields)))
            }
            JsValue::Builtin(Builtin::DOMParserCtor) => JsValue::Builtin(Builtin::DOMParser),
            JsValue::Builtin(Builtin::XMLSerializerCtor) => JsValue::Builtin(Builtin::XMLSerializer),
            JsValue::Builtin(
                builtin @ (Builtin::HeadersCtor
                | Builtin::RequestCtor
                | Builtin::ResponseCtor
                | Builtin::AbortControllerCtor
                | Builtin::URLCtor
                | Builtin::URLSearchParamsCtor
                | Builtin::AudioContextCtor
                | Builtin::IntersectionObserverCtor
                | Builtin::MapCtor
                | Builtin::SetCtor),
            ) => self.construct_host(*builtin, args),
            other => {
                let description = to_string(other);
                self.throw_type_error(format!("{description} is not a constructor"));
                JsValue::Undefined
            }
        }
    }

    /// Turn any value into a promise: promises pass through, everything else
    /// becomes an already-fulfilled promise.
    fn promise_from(&mut self, value: JsValue) -> PromiseRef {
        match value {
            JsValue::Promise(existing) => existing,
            other => promise::fulfilled_promise(other),
        }
    }

    /// `promise.then(...)`, `.catch(...)` and `.finally(...)`.
    fn promise_method(&mut self, target: &PromiseRef, prop: &str, args: Vec<JsValue>) -> JsValue {
        let callable = |value: Option<&JsValue>| value.filter(|v| is_callable(v)).cloned();

        let (child, tasks) = match prop {
            "then" => promise::then(
                target,
                callable(args.first()),
                callable(args.get(1)),
                ReactionKind::Then,
            ),
            // `catch(f)` is `then(undefined, f)`.
            "catch" => promise::then(target, None, callable(args.first()), ReactionKind::Then),
            // `finally(f)` runs `f` either way and passes the outcome through.
            "finally" => {
                let handler = callable(args.first());
                promise::then(target, handler.clone(), handler, ReactionKind::Finally)
            }
            _ => return JsValue::Undefined,
        };
        self.queue_microtasks(tasks);
        JsValue::Promise(child)
    }

    /// The `Promise.*` static methods.
    fn promise_static(&mut self, prop: &str, args: Vec<JsValue>) -> JsValue {
        let first = args.first().cloned().unwrap_or(JsValue::Undefined);
        match prop {
            "resolve" => match first {
                // `Promise.resolve(p)` hands back the same promise.
                JsValue::Promise(existing) => JsValue::Promise(existing),
                other => JsValue::Promise(promise::fulfilled_promise(other)),
            },
            "reject" => JsValue::Promise(promise::rejected_promise(first)),
            "all" => self.promise_combinator(CombinatorKind::All, first),
            "race" => self.promise_combinator(CombinatorKind::Race, first),
            "allSettled" => self.promise_combinator(CombinatorKind::AllSettled, first),
            "any" => self.promise_combinator(CombinatorKind::Any, first),
            _ => JsValue::Undefined,
        }
    }

    /// `Promise.all` / `race` / `allSettled` / `any` over an array.
    ///
    /// Each entry gets a pair of native handlers that write into a shared slot
    /// vector, so the result keeps the *input* order however the entries
    /// settle.
    fn promise_combinator(&mut self, kind: CombinatorKind, input: JsValue) -> JsValue {
        let JsValue::Array(items) = input else {
            return JsValue::Promise(promise::rejected_promise(JsValue::Str(
                "TypeError: argument is not iterable".into(),
            )));
        };
        let entries: Vec<JsValue> = items.borrow().clone();
        let result = promise::new_promise();
        let count = entries.len();

        if count == 0 {
            match kind {
                // An empty `all`/`allSettled` fulfils immediately with [].
                CombinatorKind::All | CombinatorKind::AllSettled => {
                    let empty = JsValue::Array(Rc::new(RefCell::new(Vec::new())));
                    self.settle_resolve(&result, empty);
                }
                CombinatorKind::Any => self.settle_reject(
                    &result,
                    JsValue::Str("AggregateError: all promises were rejected".into()),
                ),
                // An empty `race` never settles, as in JavaScript.
                CombinatorKind::Race => {}
            }
            return JsValue::Promise(result);
        }

        let slots = Rc::new(RefCell::new(vec![JsValue::Undefined; count]));
        let remaining = Rc::new(RefCell::new(count));

        for (index, entry) in entries.into_iter().enumerate() {
            let source = self.promise_from(entry);
            let make = |rejected: bool| {
                JsValue::Combinator(Rc::new(CombinatorHandler {
                    kind,
                    index,
                    rejected,
                    slots: slots.clone(),
                    remaining: remaining.clone(),
                    result: result.clone(),
                }))
            };
            let (_, tasks) = promise::then(
                &source,
                Some(make(false)),
                Some(make(true)),
                ReactionKind::Then,
            );
            self.queue_microtasks(tasks);
        }
        JsValue::Promise(result)
    }

    /// Apply one settled entry of a `Promise.all`-style combinator.
    fn run_combinator(&mut self, handler: &CombinatorHandler, value: JsValue) -> JsValue {
        match (handler.kind, handler.rejected) {
            // `all`: collect until every entry has fulfilled; any rejection
            // settles the whole thing.
            (CombinatorKind::All, false) => {
                handler.slots.borrow_mut()[handler.index] = value;
                let done = {
                    let mut remaining = handler.remaining.borrow_mut();
                    *remaining -= 1;
                    *remaining == 0
                };
                if done {
                    let values = handler.slots.borrow().clone();
                    self.settle_resolve(
                        &handler.result,
                        JsValue::Array(Rc::new(RefCell::new(values))),
                    );
                }
            }
            (CombinatorKind::All, true) => self.settle_reject(&handler.result, value),

            // `race`: the first settlement of either kind wins.
            (CombinatorKind::Race, false) => self.settle_resolve(&handler.result, value),
            (CombinatorKind::Race, true) => self.settle_reject(&handler.result, value),

            // `allSettled`: never rejects; every entry becomes a record.
            (CombinatorKind::AllSettled, rejected) => {
                let record = Rc::new(RefCell::new(if rejected {
                    vec![
                        ("status".to_string(), JsValue::Str("rejected".into())),
                        ("reason".to_string(), value),
                    ]
                } else {
                    vec![
                        ("status".to_string(), JsValue::Str("fulfilled".into())),
                        ("value".to_string(), value),
                    ]
                }));
                handler.slots.borrow_mut()[handler.index] = JsValue::Object(record);
                let done = {
                    let mut remaining = handler.remaining.borrow_mut();
                    *remaining -= 1;
                    *remaining == 0
                };
                if done {
                    let values = handler.slots.borrow().clone();
                    self.settle_resolve(
                        &handler.result,
                        JsValue::Array(Rc::new(RefCell::new(values))),
                    );
                }
            }

            // `any`: the first fulfilment wins; it only rejects if every
            // entry rejected.
            (CombinatorKind::Any, false) => self.settle_resolve(&handler.result, value),
            (CombinatorKind::Any, true) => {
                let done = {
                    let mut remaining = handler.remaining.borrow_mut();
                    *remaining -= 1;
                    *remaining == 0
                };
                if done {
                    self.settle_reject(
                        &handler.result,
                        JsValue::Str("AggregateError: all promises were rejected".into()),
                    );
                }
            }
        }
        JsValue::Undefined
    }

    // ── Entry points ──────────────────────────────────────────────────────

    /// Parse and execute one script against `dom`.
    pub fn run_script(&mut self, dom: &mut Node, source: &str) {
        let program = Parser::new(source).parse_program();
        let scope = self.global.clone();
        self.exec_block(dom, &program, &scope);
        self.report_uncaught();
    }

    /// Call a function value with `args`.
    pub fn call_value(&mut self, dom: &mut Node, callee: &JsValue, args: Vec<JsValue>) -> JsValue {
        match callee {
            JsValue::Function(func) => {
                if self.depth >= MAX_CALL_DEPTH {
                    self.log("RangeError: maximum call stack size exceeded");
                    return JsValue::Undefined;
                }
                let scope = Scope::new(Some(func.scope.clone()));
                let mut arg_idx = 0;
                for param in &func.params {
                    if let Some(rest_name) = param.strip_prefix("...") {
                        let rest_items = if arg_idx < args.len() {
                            args[arg_idx..].to_vec()
                        } else {
                            Vec::new()
                        };
                        scope_declare(
                            &scope,
                            rest_name,
                            JsValue::Array(Rc::new(RefCell::new(rest_items))),
                        );
                        break;
                    } else {
                        scope_declare(
                            &scope,
                            param,
                            args.get(arg_idx).cloned().unwrap_or(JsValue::Undefined),
                        );
                        arg_idx += 1;
                    }
                }
                self.depth += 1;
                let flow = self.exec_block(dom, &func.body, &scope);
                self.depth -= 1;
                match flow {
                    Flow::Return(v) => v,
                    _ => JsValue::Undefined,
                }
            }
            JsValue::Builtin(b) => self.call_builtin_fn(*b, args),
            // The `resolve`/`reject` pair from a `new Promise` executor.
            JsValue::PromiseResolver { promise, reject } => {
                let target = promise.clone();
                let value = args.into_iter().next().unwrap_or(JsValue::Undefined);
                if *reject {
                    self.settle_reject(&target, value);
                } else {
                    self.settle_resolve(&target, value);
                }
                JsValue::Undefined
            }
            JsValue::Combinator(handler) => {
                let handler = handler.clone();
                let value = args.into_iter().next().unwrap_or(JsValue::Undefined);
                self.run_combinator(&handler, value)
            }
            other => {
                let kind = type_of(other).to_string();
                self.throw_type_error(format!("{kind} is not a function"));
                JsValue::Undefined
            }
        }
    }

    /// Fire `event_type` at `target`, then bubble up through its ancestors.
    pub fn dispatch_event(
        &mut self,
        dom: &mut Node,
        target: &[usize],
        event_type: &str,
    ) -> EventOutcome {
        self.dispatch_event_init(dom, target, event_type, EventInit::bubbling())
    }

    /// Dispatch an event, controlling whether it bubbles and what extra
    /// properties the event object carries (`key`, `shiftKey`, …).
    pub fn dispatch_event_init(
        &mut self,
        dom: &mut Node,
        target: &[usize],
        event_type: &str,
        init: EventInit,
    ) -> EventOutcome {
        let mut properties = vec![
            ("type".to_string(), JsValue::Str(event_type.to_string())),
            (
                "target".to_string(),
                JsValue::Element(NodeRef::Tree(target.to_vec())),
            ),
            ("bubbles".to_string(), JsValue::Bool(init.bubbles)),
            ("cancelable".to_string(), JsValue::Bool(true)),
            ("defaultPrevented".to_string(), JsValue::Bool(false)),
            ("cancelBubble".to_string(), JsValue::Bool(false)),
            ("stopImmediate".to_string(), JsValue::Bool(false)),
            ("eventPhase".to_string(), JsValue::Number(0.0)),
        ];
        properties.extend(init.fields);
        let event = Rc::new(RefCell::new(properties));
        let mut outcome = EventOutcome::default();

        let set_phase_and_target = |ev: &Rc<RefCell<Vec<(String, JsValue)>>>, phase: f32, path: &[usize]| {
            let mut props = ev.borrow_mut();
            if let Some((_, v)) = props.iter_mut().find(|(k, _)| k == "eventPhase") {
                *v = JsValue::Number(phase);
            } else {
                props.push(("eventPhase".to_string(), JsValue::Number(phase)));
            }
            if let Some((_, v)) = props.iter_mut().find(|(k, _)| k == "currentTarget") {
                *v = JsValue::Element(NodeRef::Tree(path.to_vec()));
            } else {
                props.push((
                    "currentTarget".to_string(),
                    JsValue::Element(NodeRef::Tree(path.to_vec())),
                ));
            }
        };

        // Phase 1: Capturing phase (Root down to target's parent)
        let capture_chain: Vec<NodePath> = (0..target.len()).map(|n| target[..n].to_vec()).collect();
        for path in capture_chain {
            set_phase_and_target(&event, 1.0, &path); // 1 = CAPTURING_PHASE

            let matching: Vec<Listener> = self
                .listeners
                .iter()
                .filter(|l| l.event == event_type && l.capture && self.tree_path_of(&l.target).as_deref() == Some(path.as_slice()))
                .cloned()
                .collect();

            for listener in matching {
                if listener.once {
                    self.listeners.retain(|l| l.id != listener.id);
                }

                self.call_reporting(
                    dom,
                    &listener.handler,
                    vec![JsValue::Object(event.clone())],
                    "event listener (capture)",
                );
                outcome.dispatched = true;
                outcome.default_prevented = matches!(
                    object_get(&event, "defaultPrevented"),
                    Some(JsValue::Bool(true))
                );

                if matches!(object_get(&event, "stopImmediate"), Some(JsValue::Bool(true))) {
                    return outcome;
                }
                if matches!(object_get(&event, "cancelBubble"), Some(JsValue::Bool(true))) {
                    break;
                }
            }

            if matches!(object_get(&event, "cancelBubble"), Some(JsValue::Bool(true))) {
                return outcome;
            }
        }

        // Phase 2: Target phase (at target)
        {
            set_phase_and_target(&event, 2.0, target); // 2 = AT_TARGET

            let matching: Vec<Listener> = self
                .listeners
                .iter()
                .filter(|l| l.event == event_type && self.tree_path_of(&l.target).as_deref() == Some(target))
                .cloned()
                .collect();

            for listener in matching {
                if listener.once {
                    self.listeners.retain(|l| l.id != listener.id);
                }

                self.call_reporting(
                    dom,
                    &listener.handler,
                    vec![JsValue::Object(event.clone())],
                    "event listener (target)",
                );
                outcome.dispatched = true;
                outcome.default_prevented = matches!(
                    object_get(&event, "defaultPrevented"),
                    Some(JsValue::Bool(true))
                );

                if matches!(object_get(&event, "stopImmediate"), Some(JsValue::Bool(true))) {
                    return outcome;
                }
                if matches!(object_get(&event, "cancelBubble"), Some(JsValue::Bool(true))) {
                    break;
                }
            }

            if matches!(object_get(&event, "cancelBubble"), Some(JsValue::Bool(true))) {
                return outcome;
            }
        }

        // Phase 3: Bubbling phase (Parent up to Root)
        if init.bubbles {
            let bubble_chain: Vec<NodePath> = (0..target.len()).rev().map(|n| target[..n].to_vec()).collect();
            for path in bubble_chain {
                set_phase_and_target(&event, 3.0, &path); // 3 = BUBBLING_PHASE

                let matching: Vec<Listener> = self
                    .listeners
                    .iter()
                    .filter(|l| l.event == event_type && !l.capture && self.tree_path_of(&l.target).as_deref() == Some(path.as_slice()))
                    .cloned()
                    .collect();

                for listener in matching {
                    if listener.once {
                        self.listeners.retain(|l| l.id != listener.id);
                    }

                    self.call_reporting(
                        dom,
                        &listener.handler,
                        vec![JsValue::Object(event.clone())],
                        "event listener (bubble)",
                    );
                    outcome.dispatched = true;
                    outcome.default_prevented = matches!(
                        object_get(&event, "defaultPrevented"),
                        Some(JsValue::Bool(true))
                    );

                    if matches!(object_get(&event, "stopImmediate"), Some(JsValue::Bool(true))) {
                        return outcome;
                    }
                    if matches!(object_get(&event, "cancelBubble"), Some(JsValue::Bool(true))) {
                        break;
                    }
                }

                if matches!(object_get(&event, "cancelBubble"), Some(JsValue::Bool(true))) {
                    return outcome;
                }
            }
        }

        outcome
    }

    fn log(&mut self, message: impl Into<String>) {
        let message = message.into();
        if !self.quiet {
            println!("[JS] {}", message);
        }
        self.console.push(message);
    }

    // ── Node handle resolution ────────────────────────────────────────────

    fn resolve_ref(&self, r: &NodeRef) -> Resolved {
        let mut current = r.clone();
        for _ in 0..16 {
            match current {
                NodeRef::Tree(path) => return Resolved::InTree(path),
                NodeRef::Detached { slot, path } => {
                    let alias = self.detached.get(slot).and_then(|s| s.alias.clone());
                    match alias {
                        Some(NodeRef::Tree(mut base)) => {
                            base.extend(path);
                            return Resolved::InTree(base);
                        }
                        Some(NodeRef::Detached {
                            slot: outer,
                            path: mut outer_path,
                        }) => {
                            outer_path.extend(path);
                            current = NodeRef::Detached {
                                slot: outer,
                                path: outer_path,
                            };
                        }
                        None => {
                            if self
                                .detached
                                .get(slot)
                                .and_then(|s| s.node.as_ref())
                                .is_none()
                            {
                                return Resolved::Gone;
                            }
                            return Resolved::InPool { slot, path };
                        }
                    }
                }
            }
        }
        Resolved::Gone
    }

    /// The document-tree path of a handle, if it currently lives in the tree.
    fn tree_path_of(&self, r: &NodeRef) -> Option<NodePath> {
        match self.resolve_ref(r) {
            Resolved::InTree(p) => Some(p),
            _ => None,
        }
    }

    fn with_node<R>(&self, dom: &Node, r: &NodeRef, f: impl FnOnce(&Node) -> R) -> Option<R> {
        match self.resolve_ref(r) {
            Resolved::InTree(path) => dom_api::node_at(dom, &path).map(f),
            Resolved::InPool { slot, path } => self.detached[slot]
                .node
                .as_ref()
                .and_then(|root| dom_api::node_at(root, &path))
                .map(f),
            Resolved::Gone => None,
        }
    }

    fn with_node_mut<R>(
        &mut self,
        dom: &mut Node,
        r: &NodeRef,
        f: impl FnOnce(&mut Node) -> R,
    ) -> Option<R> {
        match self.resolve_ref(r) {
            Resolved::InTree(path) => dom_api::node_at_mut(dom, &path).map(f),
            Resolved::InPool { slot, path } => self.detached[slot]
                .node
                .as_mut()
                .and_then(|root| dom_api::node_at_mut(root, &path))
                .map(f),
            Resolved::Gone => None,
        }
    }

    /// Read-only access to the element data behind a handle.
    fn with_element<R>(
        &self,
        dom: &Node,
        r: &NodeRef,
        f: impl FnOnce(&crate::dom::ElementData) -> R,
    ) -> Option<R> {
        self.with_node(dom, r, |n| n.as_element().map(f)).flatten()
    }

    fn with_element_mut<R>(
        &mut self,
        dom: &mut Node,
        r: &NodeRef,
        f: impl FnOnce(&mut crate::dom::ElementData) -> R,
    ) -> Option<R> {
        self.with_node_mut(dom, r, |n| match &mut n.node_type {
            NodeType::Element(e) => Some(f(e)),
            _ => None,
        })
        .flatten()
    }

    /// Detach the node behind `r` from wherever it currently lives.
    fn take_node(&mut self, dom: &mut Node, r: &NodeRef) -> Option<Node> {
        match self.resolve_ref(r) {
            Resolved::InTree(path) => remove_node_at(dom, &path),
            Resolved::InPool { slot, path } => {
                if path.is_empty() {
                    self.detached[slot].node.take()
                } else {
                    let root = self.detached[slot].node.as_mut()?;
                    remove_node_at(root, &path)
                }
            }
            Resolved::Gone => None,
        }
    }

    // ── Statements ────────────────────────────────────────────────────────

    fn exec_block(&mut self, dom: &mut Node, stmts: &[Stmt], scope: &ScopeRef) -> Flow {
        // Hoist function declarations so they can be called before their definition.
        for stmt in stmts {
            if let Stmt::FnDecl { name, params, body } = stmt {
                let f = JsValue::Function(Rc::new(FunctionValue {
                    params: params.clone(),
                    body: body.clone(),
                    scope: scope.clone(),
                }));
                scope_declare(scope, name, f);
            }
        }
        for stmt in stmts {
            match self.exec_stmt(dom, stmt, scope) {
                Flow::Normal => {
                    // An expression statement can throw without producing a
                    // `Flow`, so the flag is the authority.
                    if self.has_exception() {
                        return Flow::Throw;
                    }
                }
                other => return other,
            }
        }
        Flow::Normal
    }

    fn exec_stmt(&mut self, dom: &mut Node, stmt: &Stmt, scope: &ScopeRef) -> Flow {
        match stmt {
            Stmt::VarDecl { name, init } => {
                let value = match init {
                    Some(e) => self.eval(dom, e, scope),
                    None => JsValue::Undefined,
                };
                scope_declare(scope, name, value);
                Flow::Normal
            }
            Stmt::DestructDecl { pattern, init } => {
                let val = self.eval(dom, init, scope);
                match pattern {
                    DestructPat::Object(fields) => {
                        for (key, binding_name) in fields {
                            let member_val = if matches!(val, JsValue::Null | JsValue::Undefined) {
                                JsValue::Undefined
                            } else {
                                self.get_member(dom, &val, key)
                            };
                            scope_declare(scope, binding_name, member_val);
                        }
                    }
                    DestructPat::Array { items, rest } => {
                        let arr_items = match &val {
                            JsValue::Array(arr) => arr.borrow().clone(),
                            JsValue::Str(s) => {
                                s.chars().map(|c| JsValue::Str(c.to_string())).collect()
                            }
                            _ => Vec::new(),
                        };
                        for (i, item) in items.iter().enumerate() {
                            if let Some(name) = item {
                                let v = arr_items.get(i).cloned().unwrap_or(JsValue::Undefined);
                                scope_declare(scope, name, v);
                            }
                        }
                        if let Some(rest_name) = rest {
                            let rest_slice = if items.len() < arr_items.len() {
                                arr_items[items.len()..].to_vec()
                            } else {
                                Vec::new()
                            };
                            scope_declare(
                                scope,
                                rest_name,
                                JsValue::Array(Rc::new(RefCell::new(rest_slice))),
                            );
                        }
                    }
                }
                Flow::Normal
            }
            Stmt::FnDecl { .. } => Flow::Normal, // handled by hoisting
            Stmt::Expr(e) => {
                self.eval(dom, e, scope);
                Flow::Normal
            }
            Stmt::Block(body) => {
                let inner = Scope::new(Some(scope.clone()));
                self.exec_block(dom, body, &inner)
            }
            Stmt::If { test, cons, alt } => {
                let inner = Scope::new(Some(scope.clone()));
                if truthy(&self.eval(dom, test, scope)) {
                    self.exec_block(dom, cons, &inner)
                } else if let Some(alt) = alt {
                    self.exec_block(dom, alt, &inner)
                } else {
                    Flow::Normal
                }
            }
            Stmt::While { test, body } => {
                let mut iterations = 0usize;
                while truthy(&self.eval(dom, test, scope)) {
                    if self.has_exception() {
                        return Flow::Throw;
                    }
                    if self.guard_loop(&mut iterations) {
                        break;
                    }
                    let inner = Scope::new(Some(scope.clone()));
                    match self.exec_block(dom, body, &inner) {
                        Flow::Break => break,
                        Flow::Return(v) => return Flow::Return(v),
                        // An exception unwinds out of the loop.
                        Flow::Throw => return Flow::Throw,
                        _ => {}
                    }
                }
                Flow::Normal
            }
            Stmt::For {
                init,
                test,
                update,
                body,
            } => {
                let loop_scope = Scope::new(Some(scope.clone()));
                if let Some(init) = init {
                    self.exec_stmt(dom, init, &loop_scope);
                }
                let mut iterations = 0usize;
                loop {
                    if let Some(test) = test {
                        let keep_going = truthy(&self.eval(dom, test, &loop_scope));
                        if self.has_exception() {
                            return Flow::Throw;
                        }
                        if !keep_going {
                            break;
                        }
                    }
                    if self.guard_loop(&mut iterations) {
                        break;
                    }
                    let inner = Scope::new(Some(loop_scope.clone()));
                    match self.exec_block(dom, body, &inner) {
                        Flow::Break => break,
                        Flow::Return(v) => return Flow::Return(v),
                        // An exception unwinds out of the loop.
                        Flow::Throw => return Flow::Throw,
                        _ => {}
                    }
                    if let Some(update) = update {
                        self.eval(dom, update, &loop_scope);
                    }
                }
                Flow::Normal
            }
            Stmt::ForOf {
                name,
                iterable,
                body,
            } => {
                let items = match self.eval(dom, iterable, scope) {
                    JsValue::Array(items) => items.borrow().clone(),
                    JsValue::Str(s) => s.chars().map(|c| JsValue::Str(c.to_string())).collect(),
                    _ => Vec::new(),
                };
                for item in items {
                    let inner = Scope::new(Some(scope.clone()));
                    scope_declare(&inner, name, item);
                    match self.exec_block(dom, body, &inner) {
                        Flow::Break => break,
                        Flow::Return(v) => return Flow::Return(v),
                        // An exception unwinds out of the loop.
                        Flow::Throw => return Flow::Throw,
                        _ => {}
                    }
                }
                Flow::Normal
            }
            Stmt::ForIn { name, target, body } => {
                let val = self.eval(dom, target, scope);
                let keys: Vec<String> = match val {
                    JsValue::Object(props) => {
                        props.borrow().iter().map(|(k, _)| k.clone()).collect()
                    }
                    JsValue::Array(items) => {
                        (0..items.borrow().len()).map(|i| i.to_string()).collect()
                    }
                    JsValue::Str(s) => (0..s.chars().count()).map(|i| i.to_string()).collect(),
                    _ => Vec::new(),
                };
                for key in keys {
                    let inner = Scope::new(Some(scope.clone()));
                    scope_declare(&inner, name, JsValue::Str(key));
                    match self.exec_block(dom, body, &inner) {
                        Flow::Break => break,
                        Flow::Return(v) => return Flow::Return(v),
                        Flow::Throw => return Flow::Throw,
                        _ => {}
                    }
                }
                Flow::Normal
            }
            Stmt::Return(value) => {
                let v = match value {
                    Some(e) => self.eval(dom, e, scope),
                    None => JsValue::Undefined,
                };
                Flow::Return(v)
            }
            Stmt::Break => Flow::Break,
            Stmt::Continue => Flow::Continue,

            Stmt::Throw(expression) => {
                let value = self.eval(dom, expression, scope);
                // Evaluating the operand may itself have thrown; that
                // exception wins.
                if !self.has_exception() {
                    self.throw_value(value);
                }
                Flow::Throw
            }

            Stmt::Try {
                block,
                catch,
                finally,
            } => {
                let inner = Scope::new(Some(scope.clone()));
                let mut flow = self.exec_block(dom, block, &inner);

                // A thrown value is handed to the catch clause, if there is one.
                if let Some(error) = self.take_exception() {
                    match catch {
                        Some((binding, handler)) => {
                            let handler_scope = Scope::new(Some(scope.clone()));
                            if let Some(name) = binding {
                                scope_declare(&handler_scope, name, error);
                            }
                            flow = self.exec_block(dom, handler, &handler_scope);
                        }
                        // No catch: the exception keeps unwinding after
                        // `finally` has run.
                        None => {
                            self.throw_value(error);
                            flow = Flow::Throw;
                        }
                    }
                }

                if let Some(cleanup) = finally {
                    // `finally` runs with the pending exception set aside, and
                    // can replace the outcome by throwing or returning itself.
                    let pending = self.take_exception();
                    let cleanup_scope = Scope::new(Some(scope.clone()));
                    let cleanup_flow = self.exec_block(dom, cleanup, &cleanup_scope);
                    match cleanup_flow {
                        Flow::Normal => {
                            if let Some(error) = pending {
                                self.throw_value(error);
                                flow = Flow::Throw;
                            }
                        }
                        // A `return`/`break`/`throw` in `finally` discards the
                        // original outcome, as in JavaScript.
                        other => flow = other,
                    }
                }
                flow
            }
        }
    }

    /// Returns true when the loop has run too long and should be abandoned.
    fn guard_loop(&mut self, iterations: &mut usize) -> bool {
        *iterations += 1;
        if *iterations > MAX_LOOP_ITERATIONS {
            self.log("script aborted: loop exceeded iteration limit");
            true
        } else {
            false
        }
    }

    // ── Expressions ───────────────────────────────────────────────────────

    /// Evaluate an argument list, stopping at the first throw.
    fn eval_args(&mut self, dom: &mut Node, args: &[Expr], scope: &ScopeRef) -> Vec<JsValue> {
        let mut values = Vec::with_capacity(args.len());
        for argument in args {
            if let Expr::Spread(inner) = argument {
                let spread_val = self.eval(dom, inner, scope);
                if self.has_exception() {
                    break;
                }
                match spread_val {
                    JsValue::Array(items) => {
                        values.extend(items.borrow().clone());
                    }
                    JsValue::Str(s) => {
                        for c in s.chars() {
                            values.push(JsValue::Str(c.to_string()));
                        }
                    }
                    _ => {}
                }
            } else {
                values.push(self.eval(dom, argument, scope));
                if self.has_exception() {
                    break;
                }
            }
        }
        values
    }

    fn eval(&mut self, dom: &mut Node, expr: &Expr, scope: &ScopeRef) -> JsValue {
        match expr {
            Expr::Num(n) => JsValue::Number(*n),
            Expr::Str(s) => JsValue::Str(s.clone()),
            Expr::Bool(b) => JsValue::Bool(*b),
            Expr::Null => JsValue::Null,
            Expr::Undefined => JsValue::Undefined,

            Expr::Ident(name) => scope_lookup(scope, name)
                .or_else(|| global_builtin(name))
                .unwrap_or(JsValue::Undefined),

            Expr::TemplateLiteral { parts, exprs } => {
                let mut out = String::new();
                for (i, part) in parts.iter().enumerate() {
                    out.push_str(part);
                    if let Some(expr) = exprs.get(i) {
                        let val = self.eval(dom, expr, scope);
                        if self.has_exception() {
                            return JsValue::Undefined;
                        }
                        out.push_str(&to_string(&val));
                    }
                }
                JsValue::Str(out)
            }

            Expr::Spread(inner) => self.eval(dom, inner, scope),

            Expr::Array(items) => {
                let mut values = Vec::new();
                for item in items {
                    if let Expr::Spread(inner) = item {
                        let spread_val = self.eval(dom, inner, scope);
                        if self.has_exception() {
                            return JsValue::Undefined;
                        }
                        match spread_val {
                            JsValue::Array(arr) => {
                                values.extend(arr.borrow().clone());
                            }
                            JsValue::Str(s) => {
                                for c in s.chars() {
                                    values.push(JsValue::Str(c.to_string()));
                                }
                            }
                            _ => {}
                        }
                    } else {
                        values.push(self.eval(dom, item, scope));
                        if self.has_exception() {
                            return JsValue::Undefined;
                        }
                    }
                }
                JsValue::Array(Rc::new(RefCell::new(values)))
            }

            Expr::Object(props) => {
                let mut values = Vec::new();
                for (k, e) in props {
                    if k == "__spread__" {
                        if let Expr::Spread(inner) = e {
                            let spread_val = self.eval(dom, inner, scope);
                            if self.has_exception() {
                                return JsValue::Undefined;
                            }
                            if let JsValue::Object(obj) = spread_val {
                                for (sub_k, sub_v) in obj.borrow().iter() {
                                    if let Some(pos) = values
                                        .iter()
                                        .position(|(existing_k, _)| existing_k == sub_k)
                                    {
                                        values[pos] = (sub_k.clone(), sub_v.clone());
                                    } else {
                                        values.push((sub_k.clone(), sub_v.clone()));
                                    }
                                }
                            }
                            continue;
                        }
                    }
                    let val = self.eval(dom, e, scope);
                    if self.has_exception() {
                        return JsValue::Undefined;
                    }
                    if let Some(pos) = values.iter().position(|(existing_k, _)| existing_k == k) {
                        values[pos] = (k.clone(), val);
                    } else {
                        values.push((k.clone(), val));
                    }
                }
                JsValue::Object(Rc::new(RefCell::new(values)))
            }

            Expr::Function { params, body } => JsValue::Function(Rc::new(FunctionValue {
                params: params.clone(),
                body: body.clone(),
                scope: scope.clone(),
            })),

            Expr::Member { obj, prop } => {
                let target = self.eval(dom, obj, scope);
                self.get_member(dom, &target, prop)
            }

            Expr::OptionalMember { obj, prop } => {
                let target = self.eval(dom, obj, scope);
                if self.has_exception() || matches!(target, JsValue::Null | JsValue::Undefined) {
                    JsValue::Undefined
                } else {
                    self.get_member(dom, &target, prop)
                }
            }

            Expr::Index { obj, index } => {
                let target = self.eval(dom, obj, scope);
                let key = self.eval(dom, index, scope);
                self.get_index(dom, &target, &key)
            }

            Expr::OptionalIndex { obj, index } => {
                let target = self.eval(dom, obj, scope);
                if self.has_exception() || matches!(target, JsValue::Null | JsValue::Undefined) {
                    JsValue::Undefined
                } else {
                    let key = self.eval(dom, index, scope);
                    if self.has_exception() {
                        JsValue::Undefined
                    } else {
                        self.get_index(dom, &target, &key)
                    }
                }
            }

            Expr::Call { callee, args } => {
                let argv = self.eval_args(dom, args, scope);
                // An argument that threw stops the call from happening.
                if self.has_exception() {
                    return JsValue::Undefined;
                }
                match &**callee {
                    Expr::Member { obj, prop } => {
                        let target = self.eval(dom, obj, scope);
                        if self.has_exception() {
                            return JsValue::Undefined;
                        }
                        self.call_method(dom, &target, prop, argv)
                    }
                    other => {
                        let f = self.eval(dom, other, scope);
                        if self.has_exception() {
                            return JsValue::Undefined;
                        }
                        self.call_value(dom, &f, argv)
                    }
                }
            }

            Expr::OptionalCall { callee, args } => match &**callee {
                Expr::Member { obj, prop } | Expr::OptionalMember { obj, prop } => {
                    let target = self.eval(dom, obj, scope);
                    if self.has_exception() || matches!(target, JsValue::Null | JsValue::Undefined)
                    {
                        return JsValue::Undefined;
                    }
                    let f = self.get_member(dom, &target, prop);
                    if self.has_exception() || matches!(f, JsValue::Null | JsValue::Undefined) {
                        return JsValue::Undefined;
                    }
                    let argv = self.eval_args(dom, args, scope);
                    if self.has_exception() {
                        return JsValue::Undefined;
                    }
                    self.call_method(dom, &target, prop, argv)
                }
                Expr::Index { obj, index } | Expr::OptionalIndex { obj, index } => {
                    let target = self.eval(dom, obj, scope);
                    if self.has_exception() || matches!(target, JsValue::Null | JsValue::Undefined)
                    {
                        return JsValue::Undefined;
                    }
                    let key = self.eval(dom, index, scope);
                    if self.has_exception() {
                        return JsValue::Undefined;
                    }
                    let f = self.get_index(dom, &target, &key);
                    if self.has_exception() || matches!(f, JsValue::Null | JsValue::Undefined) {
                        return JsValue::Undefined;
                    }
                    let argv = self.eval_args(dom, args, scope);
                    if self.has_exception() {
                        return JsValue::Undefined;
                    }
                    self.call_value(dom, &f, argv)
                }
                other => {
                    let f = self.eval(dom, other, scope);
                    if self.has_exception() || matches!(f, JsValue::Null | JsValue::Undefined) {
                        return JsValue::Undefined;
                    }
                    let argv = self.eval_args(dom, args, scope);
                    if self.has_exception() {
                        return JsValue::Undefined;
                    }
                    self.call_value(dom, &f, argv)
                }
            },

            Expr::Unary { op, expr } => {
                let v = self.eval(dom, expr, scope);
                match op {
                    UnaryOp::Neg => JsValue::Number(-to_number(&v)),
                    UnaryOp::Not => JsValue::Bool(!truthy(&v)),
                    UnaryOp::Typeof => JsValue::Str(type_of(&v).to_string()),
                }
            }

            Expr::Binary { op, lhs, rhs } => {
                let l = self.eval(dom, lhs, scope);
                if self.has_exception() {
                    return JsValue::Undefined;
                }
                let r = self.eval(dom, rhs, scope);
                if self.has_exception() {
                    return JsValue::Undefined;
                }
                binary_op(*op, &l, &r)
            }

            Expr::Logical { op, lhs, rhs } => {
                let l = self.eval(dom, lhs, scope);
                match op {
                    LogicalOp::And => {
                        if truthy(&l) {
                            self.eval(dom, rhs, scope)
                        } else {
                            l
                        }
                    }
                    LogicalOp::Or => {
                        if truthy(&l) {
                            l
                        } else {
                            self.eval(dom, rhs, scope)
                        }
                    }
                    LogicalOp::NullishCoalescing => {
                        if matches!(l, JsValue::Null | JsValue::Undefined) {
                            self.eval(dom, rhs, scope)
                        } else {
                            l
                        }
                    }
                }
            }

            Expr::Cond { test, cons, alt } => {
                if truthy(&self.eval(dom, test, scope)) {
                    self.eval(dom, cons, scope)
                } else {
                    self.eval(dom, alt, scope)
                }
            }

            Expr::Assign { target, op, value } => {
                let rhs = self.eval(dom, value, scope);
                let result = match op {
                    AssignOp::Assign => rhs,
                    _ => {
                        let current = self.eval(dom, target, scope);
                        let bin = match op {
                            AssignOp::Add => BinOp::Add,
                            AssignOp::Sub => BinOp::Sub,
                            AssignOp::Mul => BinOp::Mul,
                            AssignOp::Div => BinOp::Div,
                            AssignOp::Assign => unreachable!(),
                        };
                        binary_op(bin, &current, &rhs)
                    }
                };
                self.assign(dom, target, result.clone(), scope);
                result
            }

            Expr::Update { target, op, prefix } => {
                let old = to_number(&self.eval(dom, target, scope));
                let new = match op {
                    UpdateOp::Inc => old + 1.0,
                    UpdateOp::Dec => old - 1.0,
                };
                self.assign(dom, target, JsValue::Number(new), scope);
                JsValue::Number(if *prefix { new } else { old })
            }

            Expr::New { callee, args } => {
                let constructor = self.eval(dom, callee, scope);
                if self.has_exception() {
                    return JsValue::Undefined;
                }
                let argv = self.eval_args(dom, args, scope);
                if self.has_exception() {
                    return JsValue::Undefined;
                }
                self.construct(dom, &constructor, argv)
            }
        }
    }

    fn assign(&mut self, dom: &mut Node, target: &Expr, value: JsValue, scope: &ScopeRef) {
        match target {
            Expr::Ident(name) => {
                if !scope_assign(scope, name, value.clone()) {
                    // Assigning to an undeclared name creates a global, as in sloppy-mode JS.
                    scope_declare(&self.global.clone(), name, value);
                }
            }
            Expr::Member { obj, prop } => {
                let target = self.eval(dom, obj, scope);
                self.set_member(dom, &target, prop, value);
            }
            Expr::Index { obj, index } => {
                let target = self.eval(dom, obj, scope);
                let key = self.eval(dom, index, scope);
                self.set_index(dom, &target, &key, value);
            }
            _ => {}
        }
    }

    // ── Property access ───────────────────────────────────────────────────

    fn get_member(&mut self, dom: &mut Node, target: &JsValue, prop: &str) -> JsValue {
        match target {
            JsValue::Str(s) => match prop {
                "length" => JsValue::Number(s.chars().count() as f32),
                _ => JsValue::Undefined,
            },
            JsValue::Array(items) => match prop {
                "length" => JsValue::Number(items.borrow().len() as f32),
                _ => JsValue::Undefined,
            },
            JsValue::Object(props) => object_get(props, prop).unwrap_or(JsValue::Undefined),
            JsValue::Element(r) => self.get_element_member(dom, r, prop),
            JsValue::Style(r) => {
                let css = dom_api::css_property_name(prop);
                self.with_element(dom, r, |e| dom_api::get_style_property(e, &css))
                    .flatten()
                    .map(JsValue::Str)
                    .unwrap_or(JsValue::Str(String::new()))
            }
            JsValue::ClassList(r) => match prop {
                "length" => JsValue::Number(
                    self.with_element(dom, r, |e| dom_api::class_list(e).len())
                        .unwrap_or(0) as f32,
                ),
                _ => JsValue::Undefined,
            },
            JsValue::Dataset(r) => {
                let attr_name = format!("data-{}", dom_api::camel_to_kebab(prop));
                self.with_element(dom, r, |e| {
                    e.get_attr(&attr_name).map(|v| JsValue::Str(v.to_string()))
                })
                .flatten()
                .unwrap_or(JsValue::Undefined)
            }
            JsValue::ComputedStyle(r) => {
                let css_prop = dom_api::camel_to_kebab(prop);
                let stylesheet = dom_api::collect_active_stylesheet(dom);
                let style_map_opt = if let Some(path) = self.tree_path_of(r) {
                    crate::style::compute_element_style(dom, &path, &stylesheet, 800.0)
                } else if let Resolved::InPool { slot, path } = self.resolve_ref(r) {
                    self.detached.get(slot).and_then(|s| s.node.as_ref()).and_then(|root| {
                        crate::style::compute_element_style(root, &path, &stylesheet, 800.0)
                    })
                } else {
                    None
                };
                if let Some(map) = style_map_opt {
                    if let Some(val) = map.get(&css_prop).or_else(|| map.get(prop)) {
                        JsValue::Str(val.to_css_string())
                    } else {
                        JsValue::Str(String::new())
                    }
                } else {
                    JsValue::Str(String::new())
                }
            }
            JsValue::Builtin(Builtin::Window) => match prop {
                "document" => JsValue::Builtin(Builtin::Document),
                "window" | "self" => JsValue::Builtin(Builtin::Window),
                "location" => JsValue::Builtin(Builtin::Location),
                "navigator" => JsValue::Builtin(Builtin::Navigator),
                "screen" => JsValue::Builtin(Builtin::Screen),
                "history" => JsValue::Builtin(Builtin::History),
                "localStorage" => JsValue::Builtin(Builtin::LocalStorage),
                "sessionStorage" => JsValue::Builtin(Builtin::SessionStorage),
                "getComputedStyle" => JsValue::Builtin(Builtin::GetComputedStyle),
                "innerWidth" | "outerWidth" => JsValue::Number(800.0),
                "innerHeight" | "outerHeight" => JsValue::Number(600.0),
                "devicePixelRatio" => JsValue::Number(1.0),
                _ => global_builtin(prop).unwrap_or(JsValue::Undefined),
            },
            JsValue::Builtin(Builtin::Document) => match prop {
                "cookie" => {
                    let jar = self.cookie_jar.borrow();
                    JsValue::Str(jar.get_document_cookie(&self.url, self.now_ms as u64))
                }
                "body" => dom_api::body_path(dom)
                    .map(|p| JsValue::Element(NodeRef::Tree(p)))
                    .unwrap_or(JsValue::Null),
                "activeElement" => {
                    match self
                        .focused
                        .and_then(|id| dom_api::path_of_element_id(dom, id))
                    {
                        Some(path) => JsValue::Element(NodeRef::Tree(path)),
                        // With nothing focused the body is the active element.
                        None => dom_api::body_path(dom)
                            .map(|p| JsValue::Element(NodeRef::Tree(p)))
                            .unwrap_or(JsValue::Null),
                    }
                }
                "documentElement" => dom_api::query_selector(dom, &[], "html")
                    .map(|p| JsValue::Element(NodeRef::Tree(p)))
                    .unwrap_or(JsValue::Null),
                "title" => dom_api::query_selector(dom, &[], "title")
                    .and_then(|p| dom_api::node_at(dom, &p))
                    .map(|n| JsValue::Str(dom_api::text_content(n)))
                    .unwrap_or(JsValue::Str(String::new())),
                _ => JsValue::Undefined,
            },
            JsValue::Builtin(Builtin::Math) => match prop {
                "PI" => JsValue::Number(std::f32::consts::PI),
                "E" => JsValue::Number(std::f32::consts::E),
                _ => JsValue::Undefined,
            },
            JsValue::Builtin(Builtin::Location) => match prop {
                "href" => JsValue::Str(self.url.to_string()),
                "origin" => JsValue::Str(format!("{}://{}", self.url.scheme(), self.url.host())),
                "protocol" => JsValue::Str(format!("{}:", self.url.scheme())),
                "host" => JsValue::Str(self.url.host().to_string()),
                "pathname" => JsValue::Str(self.url.path().to_string()),
                _ => JsValue::Undefined,
            },
            JsValue::Builtin(Builtin::Navigator) => match prop {
                "userAgent" => JsValue::Str("BrowserEngineToy/0.1.0 (Rust)".to_string()),
                "language" => JsValue::Str("en-US".to_string()),
                "onLine" => JsValue::Bool(true),
                _ => JsValue::Undefined,
            },
            JsValue::Builtin(Builtin::Screen) => match prop {
                "width" => JsValue::Number(1920.0),
                "height" => JsValue::Number(1080.0),
                "colorDepth" => JsValue::Number(24.0),
                _ => JsValue::Undefined,
            },
            JsValue::Builtin(Builtin::EventCtor | Builtin::CustomEventCtor) => match prop {
                "NONE" => JsValue::Number(0.0),
                "CAPTURING_PHASE" => JsValue::Number(1.0),
                "AT_TARGET" => JsValue::Number(2.0),
                "BUBBLING_PHASE" => JsValue::Number(3.0),
                _ => JsValue::Undefined,
            },
            JsValue::Builtin(Builtin::LocalStorage | Builtin::SessionStorage) => {
                let storage = if matches!(target, JsValue::Builtin(Builtin::LocalStorage)) {
                    self.local_storage.clone()
                } else {
                    self.session_storage.clone()
                };
                match prop {
                    "length" => JsValue::Number(storage.borrow().len() as f32),
                    "setItem" | "getItem" | "removeItem" | "clear" | "key" => JsValue::Undefined,
                    _ => {
                        if let Some((_, val)) = storage.borrow().iter().find(|(k, _)| k == prop) {
                            JsValue::Str(val.clone())
                        } else {
                            JsValue::Undefined
                        }
                    }
                }
            }
            JsValue::Builtin(Builtin::History) => match prop {
                "length" => JsValue::Number(1.0),
                _ => JsValue::Undefined,
            },
            JsValue::Host(host) => self.host_member(&host.clone(), prop),
            _ => JsValue::Undefined,
        }
    }

    fn get_element_member(&mut self, dom: &mut Node, r: &NodeRef, prop: &str) -> JsValue {
        match prop {
            "style" => return JsValue::Style(r.clone()),
            "classList" => return JsValue::ClassList(r.clone()),
            "dataset" => return JsValue::Dataset(r.clone()),
            "body" | "head" | "documentElement" => {
                let doc_root = match self.resolve_ref(r) {
                    Resolved::InTree(_) => Some(dom as &Node),
                    Resolved::InPool { slot, .. } => self.detached.get(slot).and_then(|s| s.node.as_ref()),
                    Resolved::Gone => None,
                };
                if let Some(root) = doc_root {
                    let sel = match prop {
                        "body" => "body",
                        "head" => "head",
                        "documentElement" => "html",
                        _ => "",
                    };
                    if let Some(p) = dom_api::query_selector(root, &[], sel) {
                        return match self.resolve_ref(r) {
                            Resolved::InTree(_) => JsValue::Element(NodeRef::Tree(p)),
                            Resolved::InPool { slot, .. } => {
                                JsValue::Element(NodeRef::Detached { slot, path: p })
                            }
                            _ => JsValue::Null,
                        };
                    }
                }
                return JsValue::Null;
            }
            "parentElement" | "parentNode" => {
                let Some(path) = self.tree_path_of(r) else {
                    return JsValue::Null;
                };
                let Some((_, parent)) = path.split_last() else {
                    return JsValue::Null;
                };
                return match dom_api::node_at(dom, parent) {
                    Some(n) if n.as_element().is_some() => {
                        JsValue::Element(NodeRef::Tree(parent.to_vec()))
                    }
                    _ => JsValue::Null,
                };
            }
            "form" => {
                let Some(path) = self.tree_path_of(r) else {
                    return JsValue::Null;
                };
                return match crate::forms::owning_form(dom, &path) {
                    Some(form) => JsValue::Element(NodeRef::Tree(form)),
                    None => JsValue::Null,
                };
            }
            "elements" => {
                let controls = match self.tree_path_of(r) {
                    Some(path) => crate::forms::form_controls(dom, &path)
                        .into_iter()
                        .map(|p| JsValue::Element(NodeRef::Tree(p)))
                        .collect(),
                    None => Vec::new(),
                };
                return JsValue::Array(Rc::new(RefCell::new(controls)));
            }
            "children" => {
                let Some(path) = self.tree_path_of(r) else {
                    return JsValue::Array(Rc::new(RefCell::new(Vec::new())));
                };
                let kids = dom_api::node_at(dom, &path)
                    .map(|n| {
                        n.children
                            .iter()
                            .enumerate()
                            .filter(|(_, c)| c.as_element().is_some())
                            .map(|(i, _)| {
                                let mut p = path.clone();
                                p.push(i);
                                JsValue::Element(NodeRef::Tree(p))
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                return JsValue::Array(Rc::new(RefCell::new(kids)));
            }
            _ => {}
        }

        let dom_view: &Node = dom;
        let value = self.with_node(dom_view, r, |node| {
            let element = node.as_element();
            match prop {
                "tagName" => element.map(|e| JsValue::Str(e.tag_name.to_uppercase())),
                "id" => element.map(|e| JsValue::Str(e.get_attr("id").unwrap_or("").to_string())),
                "className" => {
                    element.map(|e| JsValue::Str(e.get_attr("class").unwrap_or("").to_string()))
                }
                // The live value, which stops mirroring the attribute once the
                // control has been edited.
                "value" => element.map(|e| JsValue::Str(e.control_value())),
                "defaultValue" => {
                    element.map(|e| JsValue::Str(e.get_attr("value").unwrap_or("").to_string()))
                }
                "checked" => element.map(|e| JsValue::Bool(e.is_checked())),
                "defaultChecked" => element.map(|e| JsValue::Bool(e.get_attr("checked").is_some())),
                "disabled" => element.map(|e| JsValue::Bool(e.is_disabled())),
                "readOnly" => element.map(|e| JsValue::Bool(e.is_readonly())),
                "placeholder" => element
                    .map(|e| JsValue::Str(e.get_attr("placeholder").unwrap_or("").to_string())),
                "name" => {
                    element.map(|e| JsValue::Str(e.get_attr("name").unwrap_or("").to_string()))
                }
                "type" => element.map(|e| {
                    JsValue::Str(if e.tag_name == "input" {
                        e.input_type()
                    } else {
                        e.tag_name.clone()
                    })
                }),
                "selectionStart" | "selectionEnd" => {
                    element.map(|e| JsValue::Number(e.caret() as f32))
                }
                "textContent" | "innerText" => Some(JsValue::Str(dom_api::text_content(node))),
                "innerHTML" => Some(JsValue::Str(dom_api::inner_html(node))),
                "outerHTML" => Some(JsValue::Str(dom_api::outer_html(node))),
                "childElementCount" => Some(JsValue::Number(
                    node.children
                        .iter()
                        .filter(|c| c.as_element().is_some())
                        .count() as f32,
                )),
                "width" => element.map(|e| {
                    if e.tag_name == "canvas" {
                        let w = e
                            .get_attr("width")
                            .and_then(|s| s.trim().trim_end_matches("px").parse::<f32>().ok())
                            .unwrap_or(300.0);
                        JsValue::Number(w)
                    } else {
                        JsValue::Number(0.0)
                    }
                }),
                "height" => element.map(|e| {
                    if e.tag_name == "canvas" {
                        let h = e
                            .get_attr("height")
                            .and_then(|s| s.trim().trim_end_matches("px").parse::<f32>().ok())
                            .unwrap_or(150.0);
                        JsValue::Number(h)
                    } else {
                        JsValue::Number(0.0)
                    }
                }),
                _ => None,
            }
        });
        value.flatten().unwrap_or(JsValue::Undefined)
    }

    fn set_member(&mut self, dom: &mut Node, target: &JsValue, prop: &str, value: JsValue) {
        match target {
            JsValue::Object(props) => object_set(props, prop, value),
            JsValue::Builtin(Builtin::LocalStorage | Builtin::SessionStorage) => {
                let storage = if matches!(target, JsValue::Builtin(Builtin::LocalStorage)) {
                    self.local_storage.clone()
                } else {
                    self.session_storage.clone()
                };
                let val_str = to_string(&value);
                let mut s = storage.borrow_mut();
                if let Some((_, v)) = s.iter_mut().find(|(k, _)| k == prop) {
                    *v = val_str;
                } else {
                    s.push((prop.to_string(), val_str));
                }
            }
            JsValue::Builtin(Builtin::Document) => match prop {
                "cookie" => {
                    let cookie_str = to_string(&value);
                    self.cookie_jar
                        .borrow_mut()
                        .set_document_cookie(&cookie_str, &self.url, self.now_ms as u64);
                }
                "title" => {
                    let text = to_string(&value);
                    if let Some(p) = dom_api::query_selector(dom, &[], "title") {
                        self.with_node_mut(dom, &NodeRef::Tree(p), |n| {
                            dom_api::set_text_content(n, &text)
                        });
                    }
                }
                _ => {}
            },
            JsValue::Style(r) => {
                let css = dom_api::css_property_name(prop);
                let text = to_string(&value);
                self.with_element_mut(dom, r, |e| dom_api::set_style_property(e, &css, &text));
            }
            JsValue::Dataset(r) => {
                let attr_name = format!("data-{}", dom_api::camel_to_kebab(prop));
                let val_str = to_string(&value);
                self.with_element_mut(dom, r, |e| {
                    e.set_attr(&attr_name, &val_str);
                });
            }
            JsValue::Element(r) => {
                let text = to_string(&value);
                match prop {
                    "textContent" | "innerText" => {
                        self.with_node_mut(dom, r, |n| dom_api::set_text_content(n, &text));
                    }
                    "innerHTML" => {
                        let nodes = dom_api::parse_fragment(&text);
                        self.with_node_mut(dom, r, |n| {
                            n.children = nodes;
                        });
                    }
                    // Assigning `.value` sets the live value; the `value`
                    // attribute (the default) keeps whatever it had.
                    "value" => {
                        self.with_element_mut(dom, r, |e| e.set_control_value(text.clone()));
                    }
                    "checked" => {
                        let checked = truthy(&value);
                        self.with_element_mut(dom, r, |e| e.set_checked(checked));
                    }
                    "disabled" | "readOnly" => {
                        let attribute = if prop == "readOnly" {
                            "readonly"
                        } else {
                            "disabled"
                        };
                        let on = truthy(&value);
                        self.with_element_mut(dom, r, |e| {
                            if on {
                                e.set_attr(attribute, "");
                            } else {
                                e.remove_attr(attribute);
                            }
                        });
                    }
                    "placeholder" | "name" | "type" => {
                        self.with_element_mut(dom, r, |e| e.set_attr(prop, &text));
                    }
                    "width" | "height" => {
                        self.with_element_mut(dom, r, |e| {
                            e.set_attr(prop, &text);
                            if e.tag_name == "canvas" {
                                if let Some(ctx) = &e.canvas {
                                    let num = text.trim().trim_end_matches("px").parse::<u32>().unwrap_or(0);
                                    if num > 0 {
                                        let mut c = ctx.borrow_mut();
                                        if prop == "width" {
                                            c.width = num;
                                        } else {
                                            c.height = num;
                                        }
                                        c.pixels = vec![0u8; (c.width * c.height * 4) as usize];
                                    }
                                }
                            }
                        });
                    }
                    "id" | "className" => {
                        let attr = if prop == "className" { "class" } else { prop };
                        self.with_element_mut(dom, r, |e| e.set_attr(attr, &text));
                    }
                    _ => {}
                }
            }
            JsValue::Host(host) => {
                if let HostObject::CanvasRenderingContext2D(ctx) = host.as_ref() {
                    let mut ctx = ctx.borrow_mut();
                    match prop {
                        "fillStyle" => {
                            if let Some(color) = crate::css::parser::parse_color(&to_string(&value)) {
                                ctx.fill_style = color;
                            }
                        }
                        "strokeStyle" => {
                            if let Some(color) = crate::css::parser::parse_color(&to_string(&value)) {
                                ctx.stroke_style = color;
                            }
                        }
                        "lineWidth" => {
                            ctx.line_width = to_number(&value);
                        }
                        "font" => {
                            let s = to_string(&value);
                            if let Some(pos) = s.find("px") {
                                if let Ok(size) = s[..pos].trim().parse::<f32>() {
                                    ctx.font_size = size;
                                }
                            } else {
                                let num = to_number(&value);
                                if num > 0.0 {
                                    ctx.font_size = num;
                                }
                            }
                        }
                        "textAlign" => match to_string(&value).as_str() {
                            "center" => ctx.text_align = crate::layout::TextAlign::Center,
                            "right" => ctx.text_align = crate::layout::TextAlign::Right,
                            _ => ctx.text_align = crate::layout::TextAlign::Left,
                        },
                        "globalAlpha" => {
                            ctx.global_alpha = to_number(&value).clamp(0.0, 1.0);
                        }
                        "filter" => {
                            ctx.set_filter(&to_string(&value));
                        }
                        _ => {}
                    }
                } else if let HostObject::URL(u_rc) = host.as_ref() {
                    let mut u = u_rc.borrow_mut();
                    let val_str = to_string(&value);
                    match prop {
                        "href" => {
                            if let Ok(new_u) = crate::net::Url::parse(&val_str) {
                                u.url = new_u;
                            }
                        }
                        "pathname" => u.url.set_path(&val_str),
                        "search" => u.url.set_query(Some(val_str)),
                        "hash" => u.url.set_fragment(Some(val_str)),
                        "protocol" => u.url.set_scheme(&val_str),
                        "host" | "hostname" => u.url.set_host(&val_str),
                        "port" => u.url.set_port(val_str.parse().ok()),
                        _ => {}
                    }
                } else if let HostObject::AudioParam(ctx, node_id, param_name) = host.as_ref() {
                    if prop == "value" {
                        let num = to_number(&value);
                        if let Some(node) = ctx.borrow_mut().get_node_mut(*node_id) {
                            match &mut node.kind {
                                crate::audio::AudioNodeKind::Oscillator { frequency, .. } if param_name == "frequency" => {
                                    frequency.set_value(num);
                                }
                                crate::audio::AudioNodeKind::Gain { gain } if param_name == "gain" => {
                                    gain.set_value(num);
                                }
                                _ => {}
                            }
                        }
                    }
                } else if let HostObject::AudioNode(ctx, node_id) = host.as_ref() {
                    if prop == "type" {
                        let type_str = to_string(&value);
                        if let Some(osc_type) = crate::audio::OscillatorType::from_str(&type_str) {
                            if let Some(node) = ctx.borrow_mut().get_node_mut(*node_id) {
                                if let crate::audio::AudioNodeKind::Oscillator { osc_type: ref mut ot, .. } = node.kind {
                                    *ot = osc_type;
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn get_index(&mut self, dom: &mut Node, target: &JsValue, key: &JsValue) -> JsValue {
        match target {
            JsValue::Array(items) => {
                let idx = to_number(key);
                if idx < 0.0 {
                    return JsValue::Undefined;
                }
                items
                    .borrow()
                    .get(idx as usize)
                    .cloned()
                    .unwrap_or(JsValue::Undefined)
            }
            JsValue::Str(s) => {
                let idx = to_number(key);
                if idx < 0.0 {
                    return JsValue::Undefined;
                }
                s.chars()
                    .nth(idx as usize)
                    .map(|c| JsValue::Str(c.to_string()))
                    .unwrap_or(JsValue::Undefined)
            }
            _ => {
                let prop = to_string(key);
                self.get_member(dom, target, &prop)
            }
        }
    }

    fn set_index(&mut self, dom: &mut Node, target: &JsValue, key: &JsValue, value: JsValue) {
        match target {
            JsValue::Array(items) => {
                let idx = to_number(key);
                if idx < 0.0 {
                    return;
                }
                let idx = idx as usize;
                let mut items = items.borrow_mut();
                if idx >= items.len() {
                    items.resize(idx + 1, JsValue::Undefined);
                }
                items[idx] = value;
            }
            _ => {
                let prop = to_string(key);
                self.set_member(dom, target, &prop, value);
            }
        }
    }

    // ── Method calls ──────────────────────────────────────────────────────

    fn call_method(
        &mut self,
        dom: &mut Node,
        target: &JsValue,
        prop: &str,
        args: Vec<JsValue>,
    ) -> JsValue {
        // The classic runtime error: `nonexistent.foo()`. Reporting it keeps a
        // broken timer callback visible instead of silently doing nothing.
        if matches!(target, JsValue::Undefined | JsValue::Null) {
            let kind = if matches!(target, JsValue::Null) {
                "null"
            } else {
                "undefined"
            };
            // A real, catchable exception rather than a console note: this is
            // what lets `try/catch` and `.catch()` see a runtime error.
            self.throw_type_error(format!("cannot call '{prop}' of {kind}"));
            return JsValue::Undefined;
        }
        match target {
            JsValue::Promise(target) => {
                let target = target.clone();
                self.promise_method(&target, prop, args)
            }
            JsValue::Builtin(Builtin::PromiseCtor) => self.promise_static(prop, args),
            JsValue::Builtin(Builtin::Console) => {
                let line = args.iter().map(to_string).collect::<Vec<_>>().join(" ");
                let prefix = match prop {
                    "warn" => "warn: ",
                    "error" => "error: ",
                    "info" => "info: ",
                    "time" | "timeEnd" => "timer: ",
                    _ => "",
                };
                self.log(format!("{}{}", prefix, line));
                JsValue::Undefined
            }
            JsValue::Builtin(Builtin::Document) => self.call_document_method(dom, prop, &args),
            JsValue::Builtin(Builtin::Math) => math_method(prop, &args),
            JsValue::Builtin(Builtin::Json) => self.json_method(prop, &args),
            JsValue::Builtin(Builtin::Performance) => match prop {
                // Event-loop time, so it is deterministic under a test clock.
                "now" => JsValue::Number(self.now_ms as f32),
                _ => JsValue::Undefined,
            },
            JsValue::Builtin(Builtin::DateMeta) => match prop {
                "now" => JsValue::Number(self.now_ms as f32),
                _ => JsValue::Undefined,
            },
            JsValue::Builtin(Builtin::Crypto) => crypto_method(prop, &args),
            JsValue::Builtin(Builtin::ObjectMeta) => match prop {
                "keys" => match args.first() {
                    Some(JsValue::Object(props)) => {
                        let keys: Vec<JsValue> = props
                            .borrow()
                            .iter()
                            .map(|(k, _)| JsValue::Str(k.clone()))
                            .collect();
                        JsValue::Array(Rc::new(RefCell::new(keys)))
                    }
                    _ => JsValue::Array(Rc::new(RefCell::new(vec![]))),
                },
                "values" => match args.first() {
                    Some(JsValue::Object(props)) => {
                        let vals: Vec<JsValue> =
                            props.borrow().iter().map(|(_, v)| v.clone()).collect();
                        JsValue::Array(Rc::new(RefCell::new(vals)))
                    }
                    _ => JsValue::Array(Rc::new(RefCell::new(vec![]))),
                },
                "entries" => match args.first() {
                    Some(JsValue::Object(props)) => {
                        let entries: Vec<JsValue> = props
                            .borrow()
                            .iter()
                            .map(|(k, v)| {
                                JsValue::Array(Rc::new(RefCell::new(vec![
                                    JsValue::Str(k.clone()),
                                    v.clone(),
                                ])))
                            })
                            .collect();
                        JsValue::Array(Rc::new(RefCell::new(entries)))
                    }
                    _ => JsValue::Array(Rc::new(RefCell::new(vec![]))),
                },
                "assign" => {
                    if let Some(target) = args.first() {
                        if let JsValue::Object(target_props) = target {
                            for source in args.iter().skip(1) {
                                if let JsValue::Object(source_props) = source {
                                    for (k, v) in source_props.borrow().iter() {
                                        object_set(target_props, k, v.clone());
                                    }
                                }
                            }
                        }
                        target.clone()
                    } else {
                        JsValue::Undefined
                    }
                }
                _ => JsValue::Undefined,
            },
            JsValue::Builtin(Builtin::Location) => match prop {
                "reload" => {
                    self.pending.push(PendingAction::Reload);
                    JsValue::Undefined
                }
                _ => JsValue::Undefined,
            },
            JsValue::Builtin(Builtin::History) => match prop {
                "back" => {
                    self.pending.push(PendingAction::Back);
                    JsValue::Undefined
                }
                "forward" => {
                    self.pending.push(PendingAction::Forward);
                    JsValue::Undefined
                }
                _ => JsValue::Undefined,
            },
            JsValue::Builtin(Builtin::LocalStorage | Builtin::SessionStorage) => {
                let storage = if matches!(target, JsValue::Builtin(Builtin::LocalStorage)) {
                    self.local_storage.clone()
                } else {
                    self.session_storage.clone()
                };
                match prop {
                    "setItem" => {
                        let k = to_string(args.first().unwrap_or(&JsValue::Undefined));
                        let v = to_string(args.get(1).unwrap_or(&JsValue::Undefined));
                        let mut s = storage.borrow_mut();
                        if let Some((_, slot)) = s.iter_mut().find(|(key, _)| key == &k) {
                            *slot = v;
                        } else {
                            s.push((k, v));
                        }
                        JsValue::Undefined
                    }
                    "getItem" => {
                        let k = to_string(args.first().unwrap_or(&JsValue::Undefined));
                        let s = storage.borrow();
                        if let Some((_, v)) = s.iter().find(|(key, _)| key == &k) {
                            JsValue::Str(v.clone())
                        } else {
                            JsValue::Null
                        }
                    }
                    "removeItem" => {
                        let k = to_string(args.first().unwrap_or(&JsValue::Undefined));
                        storage.borrow_mut().retain(|(key, _)| key != &k);
                        JsValue::Undefined
                    }
                    "clear" => {
                        storage.borrow_mut().clear();
                        JsValue::Undefined
                    }
                    "key" => {
                        let idx = to_number(args.first().unwrap_or(&JsValue::Undefined)) as usize;
                        let s = storage.borrow();
                        if idx < s.len() {
                            JsValue::Str(s[idx].0.clone())
                        } else {
                            JsValue::Null
                        }
                    }
                    _ => JsValue::Undefined,
                }
            }
            JsValue::Builtin(Builtin::DOMParser) => match prop {
                "parseFromString" => {
                    let html_str = to_string(args.first().unwrap_or(&JsValue::Undefined));
                    let doc_node = crate::html::parse_html(&html_str);
                    let slot = self.push_detached(doc_node);
                    JsValue::Element(NodeRef::Detached {
                        slot,
                        path: Vec::new(),
                    })
                }
                _ => JsValue::Undefined,
            },
            JsValue::Builtin(Builtin::XMLSerializer) => match prop {
                "serializeToString" => {
                    if let Some(JsValue::Element(r)) = args.first() {
                        let html = self
                            .with_node(dom, r, |n| dom_api::outer_html(n))
                            .unwrap_or_default();
                        JsValue::Str(html)
                    } else {
                        JsValue::Str(String::new())
                    }
                }
                _ => JsValue::Undefined,
            },
            JsValue::Str(s) => string_method(s, prop, &args),
            JsValue::Number(n) => number_method(*n, prop, &args),
            JsValue::Array(items) => self.array_method(dom, items, prop, args),
            JsValue::Object(props) => match prop {
                "preventDefault" => {
                    if matches!(object_get(props, "cancelable"), Some(JsValue::Bool(true)) | None) {
                        object_set(props, "defaultPrevented", JsValue::Bool(true));
                    }
                    JsValue::Undefined
                }
                "stopPropagation" => {
                    object_set(props, "cancelBubble", JsValue::Bool(true));
                    JsValue::Undefined
                }
                "stopImmediatePropagation" => {
                    object_set(props, "cancelBubble", JsValue::Bool(true));
                    object_set(props, "stopImmediate", JsValue::Bool(true));
                    JsValue::Undefined
                }
                "initEvent" => {
                    let type_name = args.first().map(to_string).unwrap_or_default();
                    let bubbles = args.get(1).map(to_boolean).unwrap_or(false);
                    let cancelable = args.get(2).map(to_boolean).unwrap_or(false);
                    object_set(props, "type", JsValue::Str(type_name));
                    object_set(props, "bubbles", JsValue::Bool(bubbles));
                    object_set(props, "cancelable", JsValue::Bool(cancelable));
                    JsValue::Undefined
                }
                "initCustomEvent" => {
                    let type_name = args.first().map(to_string).unwrap_or_default();
                    let bubbles = args.get(1).map(to_boolean).unwrap_or(false);
                    let cancelable = args.get(2).map(to_boolean).unwrap_or(false);
                    let detail = args.get(3).cloned().unwrap_or(JsValue::Null);
                    object_set(props, "type", JsValue::Str(type_name));
                    object_set(props, "bubbles", JsValue::Bool(bubbles));
                    object_set(props, "cancelable", JsValue::Bool(cancelable));
                    object_set(props, "detail", detail);
                    JsValue::Undefined
                }
                _ => {
                    // Calling a function stored on an object: `handlers.onClick()`.
                    let f = object_get(props, prop).unwrap_or(JsValue::Undefined);
                    self.call_value(dom, &f, args)
                }
            },
            JsValue::Style(r) => match prop {
                "setProperty" => {
                    let name = dom_api::css_property_name(&to_string(
                        args.first().unwrap_or(&JsValue::Undefined),
                    ));
                    let value = to_string(args.get(1).unwrap_or(&JsValue::Undefined));
                    self.with_element_mut(dom, r, |e| {
                        dom_api::set_style_property(e, &name, &value)
                    });
                    JsValue::Undefined
                }
                "getPropertyValue" => {
                    let name = dom_api::css_property_name(&to_string(
                        args.first().unwrap_or(&JsValue::Undefined),
                    ));
                    self.with_element(dom, r, |e| dom_api::get_style_property(e, &name))
                        .flatten()
                        .map(JsValue::Str)
                        .unwrap_or(JsValue::Str(String::new()))
                }
                "removeProperty" => {
                    let name = dom_api::css_property_name(&to_string(
                        args.first().unwrap_or(&JsValue::Undefined),
                    ));
                    self.with_element_mut(dom, r, |e| dom_api::set_style_property(e, &name, ""));
                    JsValue::Undefined
                }
                _ => JsValue::Undefined,
            },
            JsValue::ClassList(r) => self.class_list_method(dom, r, prop, &args),
            JsValue::ComputedStyle(r) => match prop {
                "getPropertyValue" => {
                    let req_prop = to_string(args.first().unwrap_or(&JsValue::Undefined));
                    let css_prop = dom_api::camel_to_kebab(&req_prop);
                    let stylesheet = dom_api::collect_active_stylesheet(dom);
                    let style_map_opt = if let Some(path) = self.tree_path_of(r) {
                        crate::style::compute_element_style(dom, &path, &stylesheet, 800.0)
                    } else if let Resolved::InPool { slot, path } = self.resolve_ref(r) {
                        self.detached.get(slot).and_then(|s| s.node.as_ref()).and_then(|root| {
                            crate::style::compute_element_style(root, &path, &stylesheet, 800.0)
                        })
                    } else {
                        None
                    };
                    if let Some(map) = style_map_opt {
                        if let Some(val) = map.get(&req_prop).or_else(|| map.get(&css_prop)) {
                            JsValue::Str(val.to_css_string())
                        } else {
                            JsValue::Str(String::new())
                        }
                    } else {
                        JsValue::Str(String::new())
                    }
                }
                _ => JsValue::Undefined,
            },
            JsValue::Builtin(Builtin::Window) => match prop {
                "getComputedStyle" => {
                    if let Some(JsValue::Element(r)) = args.first() {
                        JsValue::ComputedStyle(r.clone())
                    } else {
                        JsValue::Undefined
                    }
                }
                _ => {
                    let target = global_builtin(prop).unwrap_or(JsValue::Undefined);
                    self.call_value(dom, &target, args)
                }
            },
            JsValue::Element(r) => self.element_method(dom, &r.clone(), prop, args),
            JsValue::Host(host) => self.host_method(&host.clone(), prop, args),
            _ => JsValue::Undefined,
        }
    }

    fn call_document_method(&mut self, dom: &mut Node, prop: &str, args: &[JsValue]) -> JsValue {
        let arg0 = to_string(args.first().unwrap_or(&JsValue::Undefined));
        match prop {
            "createEvent" => {
                let props = Rc::new(RefCell::new(vec![
                    ("type".to_string(), JsValue::Str("".into())),
                    ("bubbles".to_string(), JsValue::Bool(false)),
                    ("cancelable".to_string(), JsValue::Bool(false)),
                    ("defaultPrevented".to_string(), JsValue::Bool(false)),
                    ("cancelBubble".to_string(), JsValue::Bool(false)),
                    ("stopImmediate".to_string(), JsValue::Bool(false)),
                    ("eventPhase".to_string(), JsValue::Number(0.0)),
                    ("target".to_string(), JsValue::Null),
                    ("currentTarget".to_string(), JsValue::Null),
                    ("detail".to_string(), JsValue::Null),
                ]));
                JsValue::Object(props)
            }
            "getElementById" => dom_api::get_element_by_id(dom, &arg0)
                .map(|p| JsValue::Element(NodeRef::Tree(p)))
                .unwrap_or(JsValue::Null),
            "querySelector" => dom_api::query_selector(dom, &[], &arg0)
                .map(|p| JsValue::Element(NodeRef::Tree(p)))
                .unwrap_or(JsValue::Null),
            "querySelectorAll" => {
                let items = dom_api::query_selector_all(dom, &[], &arg0)
                    .into_iter()
                    .map(|p| JsValue::Element(NodeRef::Tree(p)))
                    .collect();
                JsValue::Array(Rc::new(RefCell::new(items)))
            }
            "getElementsByTagName" => {
                let items = dom_api::query_selector_all(dom, &[], &arg0)
                    .into_iter()
                    .map(|p| JsValue::Element(NodeRef::Tree(p)))
                    .collect();
                JsValue::Array(Rc::new(RefCell::new(items)))
            }
            "getElementsByClassName" => {
                let items = dom_api::query_selector_all(dom, &[], &format!(".{}", arg0))
                    .into_iter()
                    .map(|p| JsValue::Element(NodeRef::Tree(p)))
                    .collect();
                JsValue::Array(Rc::new(RefCell::new(items)))
            }
            "createElement" => {
                let slot = self.push_detached(Node::element(arg0.to_lowercase(), Vec::new()));
                JsValue::Element(NodeRef::Detached {
                    slot,
                    path: Vec::new(),
                })
            }
            "createTextNode" => {
                let slot = self.push_detached(Node::text(arg0));
                JsValue::Element(NodeRef::Detached {
                    slot,
                    path: Vec::new(),
                })
            }
            _ => JsValue::Undefined,
        }
    }

    fn push_detached(&mut self, node: Node) -> usize {
        self.detached.push(DetachedSlot {
            node: Some(node),
            alias: None,
        });
        self.detached.len() - 1
    }

    fn element_method(
        &mut self,
        dom: &mut Node,
        r: &NodeRef,
        prop: &str,
        args: Vec<JsValue>,
    ) -> JsValue {
        let arg0 = to_string(args.first().unwrap_or(&JsValue::Undefined));
        match prop {
            "getContext" => {
                if arg0 == "2d" {
                    let ctx = self.with_element_mut(dom, r, |e| {
                        if e.tag_name == "canvas" {
                            Some(e.canvas_context())
                        } else {
                            None
                        }
                    });
                    if let Some(Some(ctx)) = ctx {
                        return JsValue::Host(Rc::new(HostObject::CanvasRenderingContext2D(ctx)));
                    }
                }
                JsValue::Null
            }
            "getAttribute" => self
                .with_element(dom, r, |e| e.get_attr(&arg0).map(|v| v.to_string()))
                .flatten()
                .map(JsValue::Str)
                .unwrap_or(JsValue::Null),
            "setAttribute" => {
                let value = to_string(args.get(1).unwrap_or(&JsValue::Undefined));
                self.with_element_mut(dom, r, |e| e.set_attr(&arg0, &value));
                JsValue::Undefined
            }
            "hasAttribute" => JsValue::Bool(
                self.with_element(dom, r, |e| e.get_attr(&arg0).is_some())
                    .unwrap_or(false),
            ),
            "removeAttribute" => {
                self.with_element_mut(dom, r, |e| {
                    e.attributes.retain(|(k, _)| !k.eq_ignore_ascii_case(&arg0))
                });
                JsValue::Undefined
            }
            "addEventListener" => {
                if let Some(handler) = args.get(1) {
                    let mut capture = false;
                    let mut once = false;
                    let mut passive = false;

                    if let Some(opt) = args.get(2) {
                        match opt {
                            JsValue::Bool(b) => capture = *b,
                            JsValue::Object(props) => {
                                if let Some(v) = object_get(props, "capture") {
                                    capture = to_boolean(&v);
                                }
                                if let Some(v) = object_get(props, "once") {
                                    once = to_boolean(&v);
                                }
                                if let Some(v) = object_get(props, "passive") {
                                    passive = to_boolean(&v);
                                }
                            }
                            _ => {}
                        }
                    }

                    let id = self.next_listener_id;
                    self.next_listener_id += 1;
                    self.listeners.push(Listener {
                        id,
                        target: r.clone(),
                        event: arg0,
                        handler: handler.clone(),
                        capture,
                        once,
                        passive,
                    });
                }
                JsValue::Undefined
            }
            "removeEventListener" => {
                let capture = match args.get(2) {
                    Some(JsValue::Bool(b)) => *b,
                    Some(JsValue::Object(props)) => {
                        object_get(props, "capture").map(|v| to_boolean(&v)).unwrap_or(false)
                    }
                    _ => false,
                };
                let handler_opt = args.get(1);
                self.listeners.retain(|l| {
                    let matches_event = l.event == arg0 && l.target == *r && l.capture == capture;
                    let matches_handler = match handler_opt {
                        Some(h) => &l.handler == h,
                        None => true,
                    };
                    !(matches_event && matches_handler)
                });
                JsValue::Undefined
            }
            "dispatchEvent" => {
                if let Some(JsValue::Object(props)) = args.first() {
                    let event_name = object_get(props, "type")
                        .map(|v| to_string(&v))
                        .unwrap_or_default();
                    if !event_name.is_empty() {
                        if let Some(path) = self.tree_path_of(r) {
                            let bubbles = object_get(props, "bubbles")
                                .map(|v| to_boolean(&v))
                                .unwrap_or(false);
                            let mut init = if bubbles {
                                EventInit::bubbling()
                            } else {
                                EventInit::non_bubbling()
                            };
                            for (k, v) in props.borrow().iter() {
                                if k != "type" && k != "bubbles" && k != "target" && k != "currentTarget" {
                                    init.fields.push((k.clone(), v.clone()));
                                }
                            }
                            let outcome = self.dispatch_event_init(dom, &path, &event_name, init);
                            return JsValue::Bool(!outcome.default_prevented);
                        }
                    }
                } else {
                    let event_name = arg0;
                    if !event_name.is_empty() {
                        if let Some(path) = self.tree_path_of(r) {
                            let outcome = self.dispatch_event(dom, &path, &event_name);
                            return JsValue::Bool(!outcome.default_prevented);
                        }
                    }
                }
                JsValue::Bool(false)
            }
            "matches" => {
                let matched = if let Some(path) = self.tree_path_of(r) {
                    dom_api::element_matches(dom, &path, &arg0)
                } else if let Resolved::InPool { slot, path } = self.resolve_ref(r) {
                    self.detached
                        .get(slot)
                        .and_then(|s| s.node.as_ref())
                        .map(|root| dom_api::element_matches(root, &path, &arg0))
                        .unwrap_or(false)
                } else {
                    false
                };
                JsValue::Bool(matched)
            }
            "closest" => {
                if let Some(path) = self.tree_path_of(r) {
                    dom_api::element_closest(dom, &path, &arg0)
                        .map(|p| JsValue::Element(NodeRef::Tree(p)))
                        .unwrap_or(JsValue::Null)
                } else if let Resolved::InPool { slot, path } = self.resolve_ref(r) {
                    if let Some(root) = self.detached.get(slot).and_then(|s| s.node.as_ref()) {
                        dom_api::element_closest(root, &path, &arg0)
                            .map(|p| JsValue::Element(NodeRef::Detached { slot, path: p }))
                            .unwrap_or(JsValue::Null)
                    } else {
                        JsValue::Null
                    }
                } else {
                    JsValue::Null
                }
            }
            "cloneNode" => {
                let deep = args.first().map(to_boolean).unwrap_or(false);
                let cloned_opt = self.with_node(dom, r, |n| dom_api::clone_node(n, deep));
                if let Some(cloned) = cloned_opt {
                    let slot = self.push_detached(cloned);
                    JsValue::Element(NodeRef::Detached {
                        slot,
                        path: Vec::new(),
                    })
                } else {
                    JsValue::Null
                }
            }
            "getElementById" => {
                if let Some(_) = self.tree_path_of(r) {
                    dom_api::get_element_by_id(dom, &arg0)
                        .map(|p| JsValue::Element(NodeRef::Tree(p)))
                        .unwrap_or(JsValue::Null)
                } else if let Resolved::InPool { slot, .. } = self.resolve_ref(r) {
                    if let Some(root) = self.detached.get(slot).and_then(|s| s.node.as_ref()) {
                        dom_api::get_element_by_id(root, &arg0)
                            .map(|p| JsValue::Element(NodeRef::Detached { slot, path: p }))
                            .unwrap_or(JsValue::Null)
                    } else {
                        JsValue::Null
                    }
                } else {
                    JsValue::Null
                }
            }
            "getElementsByTagName" => {
                let items = if let Some(scope) = self.tree_path_of(r) {
                    dom_api::query_selector_all(dom, &scope, &arg0)
                        .into_iter()
                        .map(|p| JsValue::Element(NodeRef::Tree(p)))
                        .collect()
                } else if let Resolved::InPool { slot, path } = self.resolve_ref(r) {
                    if let Some(root) = self.detached.get(slot).and_then(|s| s.node.as_ref()) {
                        dom_api::query_selector_all(root, &path, &arg0)
                            .into_iter()
                            .map(|p| JsValue::Element(NodeRef::Detached { slot, path: p }))
                            .collect()
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                };
                JsValue::Array(Rc::new(RefCell::new(items)))
            }
            "getElementsByClassName" => {
                let items = if let Some(scope) = self.tree_path_of(r) {
                    dom_api::query_selector_all(dom, &scope, &format!(".{}", arg0))
                        .into_iter()
                        .map(|p| JsValue::Element(NodeRef::Tree(p)))
                        .collect()
                } else if let Resolved::InPool { slot, path } = self.resolve_ref(r) {
                    if let Some(root) = self.detached.get(slot).and_then(|s| s.node.as_ref()) {
                        dom_api::query_selector_all(root, &path, &format!(".{}", arg0))
                            .into_iter()
                            .map(|p| JsValue::Element(NodeRef::Detached { slot, path: p }))
                            .collect()
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                };
                JsValue::Array(Rc::new(RefCell::new(items)))
            }
            "querySelector" => {
                if let Some(scope) = self.tree_path_of(r) {
                    dom_api::query_selector(dom, &scope, &arg0)
                        .map(|p| JsValue::Element(NodeRef::Tree(p)))
                        .unwrap_or(JsValue::Null)
                } else if let Resolved::InPool { slot, path } = self.resolve_ref(r) {
                    if let Some(root) = self.detached.get(slot).and_then(|s| s.node.as_ref()) {
                        dom_api::query_selector(root, &path, &arg0)
                            .map(|p| JsValue::Element(NodeRef::Detached { slot, path: p }))
                            .unwrap_or(JsValue::Null)
                    } else {
                        JsValue::Null
                    }
                } else {
                    JsValue::Null
                }
            }
            "querySelectorAll" => {
                let items = if let Some(scope) = self.tree_path_of(r) {
                    dom_api::query_selector_all(dom, &scope, &arg0)
                        .into_iter()
                        .map(|p| JsValue::Element(NodeRef::Tree(p)))
                        .collect()
                } else if let Resolved::InPool { slot, path } = self.resolve_ref(r) {
                    if let Some(root) = self.detached.get(slot).and_then(|s| s.node.as_ref()) {
                        dom_api::query_selector_all(root, &path, &arg0)
                            .into_iter()
                            .map(|p| JsValue::Element(NodeRef::Detached { slot, path: p }))
                            .collect()
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                };
                JsValue::Array(Rc::new(RefCell::new(items)))
            }
            "appendChild" => match args.first() {
                Some(JsValue::Element(child)) => self.append_child(dom, r, &child.clone()),
                _ => JsValue::Undefined,
            },
            "removeChild" => match args.first() {
                Some(JsValue::Element(child)) => {
                    let child = child.clone();
                    self.take_node(dom, &child);
                    JsValue::Undefined
                }
                _ => JsValue::Undefined,
            },
            "remove" => {
                self.take_node(dom, r);
                JsValue::Undefined
            }
            "contains" => match args.first() {
                Some(JsValue::Element(other)) => {
                    if let (Some(a), Some(b)) = (self.tree_path_of(r), self.tree_path_of(other)) {
                        JsValue::Bool(b.starts_with(&a))
                    } else if let (
                        Resolved::InPool { slot: s1, path: p1 },
                        Resolved::InPool { slot: s2, path: p2 },
                    ) = (self.resolve_ref(r), self.resolve_ref(other))
                    {
                        JsValue::Bool(s1 == s2 && p2.starts_with(&p1))
                    } else {
                        JsValue::Bool(false)
                    }
                }
                _ => JsValue::Bool(false),
            },
            "focus" | "blur" => {
                let element_id = self.with_element(dom, r, |e| e.element_id());
                if let Some(element_id) = element_id {
                    self.pending.push(if prop == "focus" {
                        PendingAction::Focus(element_id)
                    } else {
                        PendingAction::Blur(element_id)
                    });
                }
                JsValue::Undefined
            }
            // `form.submit()` bypasses the submit event; `form.reset()` restores
            // every control to its attribute defaults.
            "submit" | "reset" => {
                let element_id =
                    self.with_element(dom, r, |e| (e.tag_name == "form").then_some(e.element_id()));
                if let Some(element_id) = element_id.flatten() {
                    self.pending.push(if prop == "submit" {
                        PendingAction::Submit(element_id)
                    } else {
                        PendingAction::Reset(element_id)
                    });
                }
                JsValue::Undefined
            }
            "click" => {
                if let Some(path) = self.tree_path_of(r) {
                    self.dispatch_event(dom, &path, "click");
                }
                JsValue::Undefined
            }
            _ => JsValue::Undefined,
        }
    }

    /// Move `child` into `parent`, returning the child handle (as the DOM does).
    fn append_child(&mut self, dom: &mut Node, parent: &NodeRef, child: &NodeRef) -> JsValue {
        // Refuse to append an element into itself or its own subtree.
        if let (Some(p), Some(c)) = (self.tree_path_of(parent), self.tree_path_of(child)) {
            if p.starts_with(&c) {
                return JsValue::Undefined;
            }
        }
        let Some(node) = self.take_node(dom, child) else {
            return JsValue::Undefined;
        };

        let destination = match self.resolve_ref(parent) {
            Resolved::InTree(path) => match dom_api::node_at_mut(dom, &path) {
                Some(target) => {
                    target.children.push(node);
                    let mut new_path = path;
                    new_path.push(target.children.len() - 1);
                    NodeRef::Tree(new_path)
                }
                None => return JsValue::Undefined,
            },
            Resolved::InPool { slot, path } => {
                let Some(root) = self.detached[slot].node.as_mut() else {
                    return JsValue::Undefined;
                };
                match dom_api::node_at_mut(root, &path) {
                    Some(target) => {
                        target.children.push(node);
                        let mut new_path = path;
                        new_path.push(target.children.len() - 1);
                        NodeRef::Detached {
                            slot,
                            path: new_path,
                        }
                    }
                    None => return JsValue::Undefined,
                }
            }
            Resolved::Gone => return JsValue::Undefined,
        };

        // Point the child's slot at its new home so existing handles stay valid.
        if let NodeRef::Detached { slot, path } = child {
            if path.is_empty() {
                self.detached[*slot].alias = Some(destination.clone());
            }
        }
        JsValue::Element(destination)
    }

    fn class_list_method(
        &mut self,
        dom: &mut Node,
        r: &NodeRef,
        prop: &str,
        args: &[JsValue],
    ) -> JsValue {
        let name = to_string(args.first().unwrap_or(&JsValue::Undefined));
        if name.is_empty() && prop != "toString" {
            return JsValue::Undefined;
        }
        let r = r.clone();
        match prop {
            "contains" => JsValue::Bool(
                self.with_element(dom, &r, |e| dom_api::class_list(e).contains(&name))
                    .unwrap_or(false),
            ),
            "add" => {
                self.with_element_mut(dom, &r, |e| {
                    let mut classes = dom_api::class_list(e);
                    if !classes.contains(&name) {
                        classes.push(name.clone());
                        dom_api::set_class_list(e, &classes);
                    }
                });
                JsValue::Undefined
            }
            "remove" => {
                self.with_element_mut(dom, &r, |e| {
                    let mut classes = dom_api::class_list(e);
                    classes.retain(|c| *c != name);
                    dom_api::set_class_list(e, &classes);
                });
                JsValue::Undefined
            }
            "toggle" => {
                let present = self
                    .with_element_mut(dom, &r, |e| {
                        let mut classes = dom_api::class_list(e);
                        let present = classes.contains(&name);
                        if present {
                            classes.retain(|c| *c != name);
                        } else {
                            classes.push(name.clone());
                        }
                        dom_api::set_class_list(e, &classes);
                        !present
                    })
                    .unwrap_or(false);
                JsValue::Bool(present)
            }
            _ => JsValue::Undefined,
        }
    }

    fn array_method(
        &mut self,
        dom: &mut Node,
        items: &Rc<RefCell<Vec<JsValue>>>,
        prop: &str,
        args: Vec<JsValue>,
    ) -> JsValue {
        match prop {
            "push" => {
                let mut v = items.borrow_mut();
                for a in args {
                    v.push(a);
                }
                JsValue::Number(v.len() as f32)
            }
            "pop" => items.borrow_mut().pop().unwrap_or(JsValue::Undefined),
            "shift" => {
                let mut v = items.borrow_mut();
                if v.is_empty() {
                    JsValue::Undefined
                } else {
                    v.remove(0)
                }
            }
            "join" => {
                let sep = args
                    .first()
                    .map(to_string)
                    .unwrap_or_else(|| ",".to_string());
                let v = items.borrow();
                JsValue::Str(v.iter().map(to_string).collect::<Vec<_>>().join(&sep))
            }
            "indexOf" => {
                let needle = args.first().cloned().unwrap_or(JsValue::Undefined);
                let v = items.borrow();
                JsValue::Number(
                    v.iter()
                        .position(|item| strict_equals(item, &needle))
                        .map(|i| i as f32)
                        .unwrap_or(-1.0),
                )
            }
            "includes" => {
                let needle = args.first().cloned().unwrap_or(JsValue::Undefined);
                let v = items.borrow();
                JsValue::Bool(v.iter().any(|item| strict_equals(item, &needle)))
            }
            "slice" => {
                let v = items.borrow();
                let start = args
                    .first()
                    .map(|a| to_number(a) as usize)
                    .unwrap_or(0)
                    .min(v.len());
                let end = args
                    .get(1)
                    .map(|a| to_number(a) as usize)
                    .unwrap_or(v.len())
                    .min(v.len())
                    .max(start);
                JsValue::Array(Rc::new(RefCell::new(v[start..end].to_vec())))
            }
            // Callback methods clone the contents first so the callback is free to
            // mutate the original array without a RefCell double borrow.
            "forEach" | "map" | "filter" => {
                let Some(callback) = args.first().cloned() else {
                    return JsValue::Undefined;
                };
                let snapshot: Vec<JsValue> = items.borrow().clone();
                let mut out = Vec::new();
                for (i, item) in snapshot.into_iter().enumerate() {
                    let result = self.call_value(
                        dom,
                        &callback,
                        vec![item.clone(), JsValue::Number(i as f32)],
                    );
                    match prop {
                        "map" => out.push(result),
                        "filter" if truthy(&result) => out.push(item),
                        _ => {}
                    }
                }
                if prop == "forEach" {
                    JsValue::Undefined
                } else {
                    JsValue::Array(Rc::new(RefCell::new(out)))
                }
            }
            _ => JsValue::Undefined,
        }
    }

    fn call_builtin_fn(&mut self, builtin: Builtin, args: Vec<JsValue>) -> JsValue {
        let arg = args.first().cloned().unwrap_or(JsValue::Undefined);
        match builtin {
            // ── Timers and frames ─────────────────────────────────────────
            // These only *register* the callback; the document decides when it
            // runs, so the interpreter never blocks or reads a clock.
            Builtin::SetTimeout | Builtin::SetInterval => {
                let Some(callback) = args.first().filter(|value| is_callable(value)).cloned()
                else {
                    self.log("TypeError: the first argument must be a function");
                    return JsValue::Number(0.0);
                };
                let delay = args.get(1).map(to_number).unwrap_or(0.0) as f64;
                let now = self.now_ms;
                let id = if builtin == Builtin::SetTimeout {
                    self.scheduler.set_timeout(callback, delay, now)
                } else {
                    self.scheduler.set_interval(callback, delay, now)
                };
                JsValue::Number(id as f32)
            }
            // Returns a pending promise and queues the request. No I/O
            // happens here: the document starts it on the next turn.
            Builtin::Fetch => self.start_fetch(args),
            Builtin::QueueMicrotask => {
                match args.first().filter(|value| is_callable(value)).cloned() {
                    Some(callback) => self.queue_microtask(Microtask::Callback(callback)),
                    None => self.throw_type_error("queueMicrotask needs a function"),
                }
                JsValue::Undefined
            }
            Builtin::GetComputedStyle => {
                if let Some(JsValue::Element(r)) = args.first() {
                    JsValue::ComputedStyle(r.clone())
                } else {
                    JsValue::Undefined
                }
            }
            Builtin::ClearTimeout | Builtin::ClearInterval => {
                if let Some(id) = task_id(&arg) {
                    self.scheduler.clear_timer(id);
                }
                JsValue::Undefined
            }
            Builtin::RequestAnimationFrame => {
                let Some(callback) = args.first().filter(|value| is_callable(value)).cloned()
                else {
                    self.log("TypeError: requestAnimationFrame needs a function");
                    return JsValue::Number(0.0);
                };
                JsValue::Number(self.scheduler.request_animation_frame(callback) as f32)
            }
            Builtin::CancelAnimationFrame => {
                if let Some(id) = task_id(&arg) {
                    self.scheduler.cancel_animation_frame(id);
                }
                JsValue::Undefined
            }
            Builtin::RequestIdleCallback => {
                let Some(callback) = args.first().filter(|value| is_callable(value)).cloned()
                else {
                    self.log("TypeError: requestIdleCallback needs a function");
                    return JsValue::Number(0.0);
                };
                let now = self.now_ms;
                let id = self.scheduler.set_timeout(callback, 0.0, now);
                JsValue::Number(id as f32)
            }
            Builtin::CancelIdleCallback => {
                if let Some(id) = task_id(&arg) {
                    self.scheduler.clear_timer(id);
                }
                JsValue::Undefined
            }
            Builtin::StructuredClone => {
                clone_value(&arg)
            }
            Builtin::ParseInt => {
                let s = to_string(&arg);
                let digits: String = s
                    .trim()
                    .chars()
                    .take_while(|c| c.is_ascii_digit() || *c == '-' || *c == '+')
                    .collect();
                JsValue::Number(digits.parse().unwrap_or(f32::NAN))
            }
            Builtin::ParseFloat => {
                let s = to_string(&arg);
                let text: String = s
                    .trim()
                    .chars()
                    .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-' || *c == '+')
                    .collect();
                JsValue::Number(text.parse().unwrap_or(f32::NAN))
            }
            Builtin::StringConv => JsValue::Str(to_string(&arg)),
            Builtin::NumberConv => JsValue::Number(to_number(&arg)),
            Builtin::BooleanConv => JsValue::Bool(truthy(&arg)),
            Builtin::IsNaN => JsValue::Bool(to_number(&arg).is_nan()),
            Builtin::EncodeUriComponent => {
                let s = to_string(&arg);
                let mut out = String::new();
                for b in s.bytes() {
                    if b.is_ascii_alphanumeric()
                        || matches!(
                            b,
                            b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
                        )
                    {
                        out.push(b as char);
                    } else {
                        out.push_str(&format!("%{:02X}", b));
                    }
                }
                JsValue::Str(out)
            }
            Builtin::DecodeUriComponent => {
                let s = to_string(&arg);
                let mut bytes = Vec::new();
                let b = s.as_bytes();
                let mut i = 0;
                while i < b.len() {
                    if b[i] == b'%' && i + 2 < b.len() {
                        if let Ok(val) = u8::from_str_radix(
                            std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""),
                            16,
                        ) {
                            bytes.push(val);
                            i += 3;
                            continue;
                        }
                    }
                    bytes.push(b[i]);
                    i += 1;
                }
                JsValue::Str(String::from_utf8_lossy(&bytes).into_owned())
            }
            Builtin::Btoa => {
                let s = to_string(&arg);
                let chars: Vec<char> =
                    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
                        .chars()
                        .collect();
                let bytes = s.as_bytes();
                let mut out = String::new();
                let mut i = 0;
                while i < bytes.len() {
                    let b0 = bytes[i] as usize;
                    let b1 = if i + 1 < bytes.len() {
                        bytes[i + 1] as usize
                    } else {
                        0
                    };
                    let b2 = if i + 2 < bytes.len() {
                        bytes[i + 2] as usize
                    } else {
                        0
                    };

                    let triplet = (b0 << 16) | (b1 << 8) | b2;

                    out.push(chars[(triplet >> 18) & 0x3F]);
                    out.push(chars[(triplet >> 12) & 0x3F]);
                    if i + 1 < bytes.len() {
                        out.push(chars[(triplet >> 6) & 0x3F]);
                    } else {
                        out.push('=');
                    }
                    if i + 2 < bytes.len() {
                        out.push(chars[triplet & 0x3F]);
                    } else {
                        out.push('=');
                    }
                    i += 3;
                }
                JsValue::Str(out)
            }
            Builtin::Atob => {
                let s = to_string(&arg);
                let lookup = |c: char| -> usize {
                    match c {
                        'A'..='Z' => (c as usize) - ('A' as usize),
                        'a'..='z' => (c as usize) - ('a' as usize) + 26,
                        '0'..='9' => (c as usize) - ('0' as usize) + 52,
                        '+' => 62,
                        '/' => 63,
                        _ => 0,
                    }
                };
                let clean: Vec<char> = s
                    .chars()
                    .filter(|c| *c != '=' && !c.is_whitespace())
                    .collect();
                let mut bytes = Vec::new();
                let mut i = 0;
                while i < clean.len() {
                    let v0 = lookup(clean[i]);
                    let v1 = if i + 1 < clean.len() {
                        lookup(clean[i + 1])
                    } else {
                        0
                    };
                    let v2 = if i + 2 < clean.len() {
                        lookup(clean[i + 2])
                    } else {
                        0
                    };
                    let v3 = if i + 3 < clean.len() {
                        lookup(clean[i + 3])
                    } else {
                        0
                    };

                    let triplet = (v0 << 18) | (v1 << 12) | (v2 << 6) | v3;

                    bytes.push(((triplet >> 16) & 0xFF) as u8);
                    if i + 2 < clean.len() {
                        bytes.push(((triplet >> 8) & 0xFF) as u8);
                    }
                    if i + 3 < clean.len() {
                        bytes.push((triplet & 0xFF) as u8);
                    }
                    i += 4;
                }
                JsValue::Str(String::from_utf8_lossy(&bytes).into_owned())
            }
            _ => JsValue::Undefined,
        }
    }
}

fn clone_value(val: &JsValue) -> JsValue {
    match val {
        JsValue::Undefined => JsValue::Undefined,
        JsValue::Null => JsValue::Null,
        JsValue::Bool(b) => JsValue::Bool(*b),
        JsValue::Number(n) => JsValue::Number(*n),
        JsValue::Str(s) => JsValue::Str(s.clone()),
        JsValue::Array(items) => {
            let cloned_items: Vec<JsValue> = items.borrow().iter().map(clone_value).collect();
            JsValue::Array(Rc::new(RefCell::new(cloned_items)))
        }
        JsValue::Object(props) => {
            let cloned_props: Vec<(String, JsValue)> = props
                .borrow()
                .iter()
                .map(|(k, v)| (k.clone(), clone_value(v)))
                .collect();
            JsValue::Object(Rc::new(RefCell::new(cloned_props)))
        }
        other => other.clone(),
    }
}

// ── Detached-tree helpers ─────────────────────────────────────────────────────

/// Remove and return the node at `path` (which must be non-empty).
fn remove_node_at(root: &mut Node, path: &[usize]) -> Option<Node> {
    let (&index, parent_path) = path.split_last()?;
    let parent = dom_api::node_at_mut(root, parent_path)?;
    if index < parent.children.len() {
        Some(parent.children.remove(index))
    } else {
        None
    }
}

// ── Object helpers ────────────────────────────────────────────────────────────

pub(crate) fn object_get(props: &Rc<RefCell<Vec<(String, JsValue)>>>, key: &str) -> Option<JsValue> {
    props
        .borrow()
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
}

fn object_set(props: &Rc<RefCell<Vec<(String, JsValue)>>>, key: &str, value: JsValue) {
    let mut p = props.borrow_mut();
    match p.iter_mut().find(|(k, _)| k == key) {
        Some((_, slot)) => *slot = value,
        None => p.push((key.to_string(), value)),
    }
}

// ── Built-in lookup ───────────────────────────────────────────────────────────

/// True for values that can be called as a function.
fn is_callable(value: &JsValue) -> bool {
    matches!(
        value,
        JsValue::Function(_)
            | JsValue::Builtin(_)
            | JsValue::PromiseResolver { .. }
            | JsValue::Combinator(_)
    )
}

/// Read a timer handle back out of a JS number.
fn task_id(value: &JsValue) -> Option<crate::eventloop::TaskId> {
    let number = to_number(value);
    if !number.is_finite() || number < 1.0 {
        return None;
    }
    Some(number as crate::eventloop::TaskId)
}

fn global_builtin(name: &str) -> Option<JsValue> {
    let builtin = match name {
        "window" | "self" => Builtin::Window,
        "document" => Builtin::Document,
        "console" => Builtin::Console,
        "Event" => Builtin::EventCtor,
        "CustomEvent" => Builtin::CustomEventCtor,
        "DOMParser" => Builtin::DOMParserCtor,
        "XMLSerializer" => Builtin::XMLSerializerCtor,
        "getComputedStyle" => Builtin::GetComputedStyle,
        "Promise" => Builtin::PromiseCtor,
        "queueMicrotask" => Builtin::QueueMicrotask,
        "fetch" => Builtin::Fetch,
        "Headers" => Builtin::HeadersCtor,
        "Request" => Builtin::RequestCtor,
        "Response" => Builtin::ResponseCtor,
        "AbortController" => Builtin::AbortControllerCtor,
        "URL" => Builtin::URLCtor,
        "URLSearchParams" => Builtin::URLSearchParamsCtor,
        "AudioContext" | "webkitAudioContext" => Builtin::AudioContextCtor,
        "localStorage" => Builtin::LocalStorage,
        "sessionStorage" => Builtin::SessionStorage,
        "Storage" => Builtin::StorageCtor,
        "setTimeout" => Builtin::SetTimeout,
        "clearTimeout" => Builtin::ClearTimeout,
        "setInterval" => Builtin::SetInterval,
        "clearInterval" => Builtin::ClearInterval,
        "requestAnimationFrame" => Builtin::RequestAnimationFrame,
        "cancelAnimationFrame" => Builtin::CancelAnimationFrame,
        "requestIdleCallback" => Builtin::RequestIdleCallback,
        "cancelIdleCallback" => Builtin::CancelIdleCallback,
        "structuredClone" => Builtin::StructuredClone,
        "Date" => Builtin::DateMeta,
        "Math" => Builtin::Math,
        "JSON" => Builtin::Json,
        "performance" => Builtin::Performance,
        "Object" => Builtin::ObjectMeta,
        "encodeURIComponent" => Builtin::EncodeUriComponent,
        "decodeURIComponent" => Builtin::DecodeUriComponent,
        "btoa" => Builtin::Btoa,
        "atob" => Builtin::Atob,
        "location" => Builtin::Location,
        "navigator" => Builtin::Navigator,
        "screen" => Builtin::Screen,
        "history" => Builtin::History,
        "parseInt" => Builtin::ParseInt,
        "parseFloat" => Builtin::ParseFloat,
        "String" => Builtin::StringConv,
        "Number" => Builtin::NumberConv,
        "Boolean" => Builtin::BooleanConv,
        "isNaN" => Builtin::IsNaN,
        "IntersectionObserver" => Builtin::IntersectionObserverCtor,
        "Map" => Builtin::MapCtor,
        "Set" => Builtin::SetCtor,
        "crypto" => Builtin::Crypto,
        _ => return None,
    };
    Some(JsValue::Builtin(builtin))
}

fn math_method(prop: &str, args: &[JsValue]) -> JsValue {
    let a = args.first().map(to_number).unwrap_or(f32::NAN);
    let b = args.get(1).map(to_number).unwrap_or(f32::NAN);
    let result = match prop {
        "floor" => a.floor(),
        "ceil" => a.ceil(),
        "round" => a.round(),
        "abs" => a.abs(),
        "sqrt" => a.sqrt(),
        "pow" => a.powf(b),
        "min" => args.iter().map(to_number).fold(f32::INFINITY, f32::min),
        "max" => args.iter().map(to_number).fold(f32::NEG_INFINITY, f32::max),
        // Deterministic pseudo-random: a real PRNG would make renders unreproducible.
        "random" => 0.5,
        _ => return JsValue::Undefined,
    };
    JsValue::Number(result)
}

fn crypto_method(prop: &str, args: &[JsValue]) -> JsValue {
    match prop {
        "getRandomValues" => {
            if let Some(arr_val) = args.first() {
                if let JsValue::Array(items) = arr_val {
                    let mut items_mut = items.borrow_mut();
                    let len = items_mut.len();
                    for i in 0..len {
                        let pseudo = ((i as u32 * 1103515245 + 12345) % 256) as f32;
                        items_mut[i] = JsValue::Number(pseudo);
                    }
                    return arr_val.clone();
                }
            }
            JsValue::Undefined
        }
        "randomUUID" => {
            let uuid = format!(
                "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
                0x110ec58a_u32,
                0xa0f2_u16,
                0xac4_u16,
                0x8393_u16,
                0xc0de00000001_u64
            );
            JsValue::Str(uuid)
        }
        _ => JsValue::Undefined,
    }
}

fn string_method(s: &str, prop: &str, args: &[JsValue]) -> JsValue {
    let arg0 = args.first().map(to_string).unwrap_or_default();
    match prop {
        "toUpperCase" => JsValue::Str(s.to_uppercase()),
        "toLowerCase" => JsValue::Str(s.to_lowercase()),
        "trim" => JsValue::Str(s.trim().to_string()),
        "includes" => JsValue::Bool(s.contains(&arg0)),
        "startsWith" => JsValue::Bool(s.starts_with(&arg0)),
        "endsWith" => JsValue::Bool(s.ends_with(&arg0)),
        "indexOf" => JsValue::Number(
            s.find(&arg0)
                .map(|i| s[..i].chars().count() as f32)
                .unwrap_or(-1.0),
        ),
        "charAt" => {
            let i = args.first().map(to_number).unwrap_or(0.0).max(0.0) as usize;
            JsValue::Str(s.chars().nth(i).map(String::from).unwrap_or_default())
        }
        "repeat" => {
            let n = args.first().map(to_number).unwrap_or(0.0).max(0.0) as usize;
            JsValue::Str(s.repeat(n.min(10_000)))
        }
        "replace" => {
            let to = args.get(1).map(to_string).unwrap_or_default();
            JsValue::Str(s.replacen(&arg0, &to, 1))
        }
        "split" => {
            let parts: Vec<JsValue> = if arg0.is_empty() {
                s.chars().map(|c| JsValue::Str(c.to_string())).collect()
            } else {
                s.split(&arg0 as &str)
                    .map(|p| JsValue::Str(p.to_string()))
                    .collect()
            };
            JsValue::Array(Rc::new(RefCell::new(parts)))
        }
        "slice" | "substring" => {
            let chars: Vec<char> = s.chars().collect();
            let start = args
                .first()
                .map(|a| to_number(a).max(0.0) as usize)
                .unwrap_or(0)
                .min(chars.len());
            let end = args
                .get(1)
                .map(|a| to_number(a).max(0.0) as usize)
                .unwrap_or(chars.len())
                .min(chars.len())
                .max(start);
            JsValue::Str(chars[start..end].iter().collect())
        }
        "toString" => JsValue::Str(s.to_string()),
        _ => JsValue::Undefined,
    }
}

fn number_method(n: f32, prop: &str, args: &[JsValue]) -> JsValue {
    match prop {
        "toFixed" => {
            let digits = args.first().map(to_number).unwrap_or(0.0).clamp(0.0, 10.0) as usize;
            JsValue::Str(format!("{:.*}", digits, n))
        }
        "toString" => JsValue::Str(number_to_string(n)),
        _ => JsValue::Undefined,
    }
}

// ── Value conversions ─────────────────────────────────────────────────────────

pub fn number_to_string(n: f32) -> String {
    if n.is_nan() {
        "NaN".to_string()
    } else if n.is_infinite() {
        if n > 0.0 {
            "Infinity".to_string()
        } else {
            "-Infinity".to_string()
        }
    } else if n.fract() == 0.0 && n.abs() < 1e9 {
        format!("{}", n as i64)
    } else {
        let s = format!("{}", n);
        s
    }
}

pub fn to_string(value: &JsValue) -> String {
    match value {
        JsValue::Undefined => "undefined".to_string(),
        JsValue::Null => "null".to_string(),
        JsValue::Bool(b) => b.to_string(),
        JsValue::Number(n) => number_to_string(*n),
        JsValue::Str(s) => s.clone(),
        JsValue::Array(items) => items
            .borrow()
            .iter()
            .map(to_string)
            .collect::<Vec<_>>()
            .join(","),
        JsValue::Object(_) => "[object Object]".to_string(),
        JsValue::Function(_) => "function".to_string(),
        JsValue::Element(_) => "[object HTMLElement]".to_string(),
        JsValue::Style(_) => "[object CSSStyleDeclaration]".to_string(),
        JsValue::ClassList(_) => "[object DOMTokenList]".to_string(),
        JsValue::Dataset(_) => "[object DOMStringMap]".to_string(),
        JsValue::ComputedStyle(_) => "[object CSSStyleDeclaration]".to_string(),
        JsValue::Builtin(b) => format!("[object {:?}]", b),
        JsValue::Promise(_) => "[object Promise]".to_string(),
        JsValue::PromiseResolver { .. } | JsValue::Combinator(_) => "function".to_string(),
        JsValue::Host(host) => format!("[object {}]", host.type_name()),
    }
}

pub fn to_number(value: &JsValue) -> f32 {
    match value {
        JsValue::Number(n) => *n,
        JsValue::Bool(true) => 1.0,
        JsValue::Bool(false) => 0.0,
        JsValue::Null => 0.0,
        JsValue::Str(s) => {
            let t = s.trim();
            if t.is_empty() {
                0.0
            } else {
                t.parse().unwrap_or(f32::NAN)
            }
        }
        _ => f32::NAN,
    }
}

pub fn truthy(value: &JsValue) -> bool {
    match value {
        JsValue::Undefined | JsValue::Null => false,
        JsValue::Bool(b) => *b,
        JsValue::Number(n) => *n != 0.0 && !n.is_nan(),
        JsValue::Str(s) => !s.is_empty(),
        _ => true,
    }
}

pub fn to_boolean(value: &JsValue) -> bool {
    truthy(value)
}

fn type_of(value: &JsValue) -> &'static str {
    match value {
        JsValue::Undefined => "undefined",
        JsValue::Null => "object",
        JsValue::Bool(_) => "boolean",
        JsValue::Number(_) => "number",
        JsValue::Str(_) => "string",
        JsValue::Function(_)
        | JsValue::Builtin(_)
        | JsValue::PromiseResolver { .. }
        | JsValue::Combinator(_) => "function",
        _ => "object",
    }
}

fn strict_equals(a: &JsValue, b: &JsValue) -> bool {
    match (a, b) {
        (JsValue::Undefined, JsValue::Undefined) => true,
        (JsValue::Null, JsValue::Null) => true,
        (JsValue::Bool(x), JsValue::Bool(y)) => x == y,
        (JsValue::Number(x), JsValue::Number(y)) => x == y,
        (JsValue::Str(x), JsValue::Str(y)) => x == y,
        (JsValue::Array(x), JsValue::Array(y)) => Rc::ptr_eq(x, y),
        (JsValue::Object(x), JsValue::Object(y)) => Rc::ptr_eq(x, y),
        (JsValue::Function(x), JsValue::Function(y)) => Rc::ptr_eq(x, y),
        (JsValue::Element(x), JsValue::Element(y)) => x == y,
        (JsValue::Builtin(x), JsValue::Builtin(y)) => x == y,
        // Promises compare by identity, like any other object.
        (JsValue::Promise(x), JsValue::Promise(y)) => Rc::ptr_eq(x, y),
        _ => false,
    }
}

impl PartialEq for JsValue {
    fn eq(&self, other: &Self) -> bool {
        strict_equals(self, other)
    }
}

fn loose_equals(a: &JsValue, b: &JsValue) -> bool {
    match (a, b) {
        (JsValue::Undefined | JsValue::Null, JsValue::Undefined | JsValue::Null) => true,
        (JsValue::Undefined | JsValue::Null, _) | (_, JsValue::Undefined | JsValue::Null) => false,
        (JsValue::Str(x), JsValue::Str(y)) => x == y,
        (JsValue::Array(_), _) | (_, JsValue::Array(_)) => strict_equals(a, b),
        (JsValue::Object(_), _) | (_, JsValue::Object(_)) => strict_equals(a, b),
        (JsValue::Element(_), _) | (_, JsValue::Element(_)) => strict_equals(a, b),
        _ => {
            let (x, y) = (to_number(a), to_number(b));
            !x.is_nan() && x == y
        }
    }
}

fn binary_op(op: BinOp, l: &JsValue, r: &JsValue) -> JsValue {
    match op {
        BinOp::Add => match (l, r) {
            // `+` concatenates when either side is a string, and adds otherwise.
            (JsValue::Str(_), _) | (_, JsValue::Str(_)) => {
                JsValue::Str(format!("{}{}", to_string(l), to_string(r)))
            }
            _ => JsValue::Number(to_number(l) + to_number(r)),
        },
        BinOp::Sub => JsValue::Number(to_number(l) - to_number(r)),
        BinOp::Mul => JsValue::Number(to_number(l) * to_number(r)),
        BinOp::Div => JsValue::Number(to_number(l) / to_number(r)),
        BinOp::Rem => JsValue::Number(to_number(l) % to_number(r)),
        BinOp::Eq => JsValue::Bool(loose_equals(l, r)),
        BinOp::NotEq => JsValue::Bool(!loose_equals(l, r)),
        BinOp::StrictEq => JsValue::Bool(strict_equals(l, r)),
        BinOp::StrictNotEq => JsValue::Bool(!strict_equals(l, r)),
        BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
            if let (JsValue::Str(x), JsValue::Str(y)) = (l, r) {
                let ordering = x.cmp(y);
                return JsValue::Bool(match op {
                    BinOp::Lt => ordering.is_lt(),
                    BinOp::Gt => ordering.is_gt(),
                    BinOp::Le => ordering.is_le(),
                    _ => ordering.is_ge(),
                });
            }
            let (x, y) = (to_number(l), to_number(r));
            if x.is_nan() || y.is_nan() {
                return JsValue::Bool(false);
            }
            JsValue::Bool(match op {
                BinOp::Lt => x < y,
                BinOp::Gt => x > y,
                BinOp::Le => x <= y,
                _ => x >= y,
            })
        }
    }
}
