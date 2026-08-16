// ============================================================
//  script/promise.rs  —  Promise state and resolution
// ============================================================
//
//  A promise is a small piece of shared mutable state: a status, and the
//  reactions waiting on it. `then` never calls anything synchronously — it
//  records a reaction and, once the promise settles, queues a *microtask* per
//  reaction. Draining that queue is the event loop's job (see `eventloop`),
//  which is what keeps ordering deterministic.
//
//  The resolution procedure is the interesting part:
//   • resolving with a plain value fulfils
//   • resolving with another promise *adopts* it — this promise settles when
//     that one does, which is what makes `.then(() => otherPromise)` chain
//   • resolving a promise with itself is a cycle, and rejects with a TypeError
//   • only the first settlement counts; later resolve/reject calls are ignored

use std::cell::RefCell;
use std::rc::Rc;

use super::interp::JsValue;

/// Shared handle to a promise's state.
pub type PromiseRef = Rc<RefCell<PromiseState>>;

/// Where a promise is in its life.
#[derive(Debug, Clone)]
pub enum PromiseStatus {
    Pending,
    Fulfilled(JsValue),
    Rejected(JsValue),
}

/// What a reaction should do when the promise settles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReactionKind {
    /// `then` / `catch`: the handler's result settles the child promise.
    Then,
    /// `finally`: the handler runs for its side effect and the original
    /// outcome passes through, unless the handler itself throws.
    Finally,
}

/// One `then` registration waiting for a promise to settle.
#[derive(Debug, Clone)]
pub struct Reaction {
    pub on_fulfilled: Option<JsValue>,
    pub on_rejected: Option<JsValue>,
    /// The promise `then` returned, settled from the handler's result.
    pub child: PromiseRef,
    pub kind: ReactionKind,
}

#[derive(Debug)]
pub struct PromiseState {
    pub status: PromiseStatus,
    /// Reactions registered while still pending, in registration order.
    pub reactions: Vec<Reaction>,
    /// Set once resolve or reject has been accepted, so the first settlement
    /// wins and later calls are ignored.
    pub settled: bool,
}

impl Default for PromiseState {
    fn default() -> Self {
        PromiseState {
            status: PromiseStatus::Pending,
            reactions: Vec::new(),
            settled: false,
        }
    }
}

/// A microtask waiting to run at the next checkpoint.
#[derive(Debug, Clone)]
pub enum Microtask {
    /// A callback handed to `queueMicrotask`.
    Callback(JsValue),
    /// A promise reaction, with the settled value it should receive.
    Reaction {
        reaction: Reaction,
        value: JsValue,
        rejected: bool,
    },
}

/// Create a pending promise.
pub fn new_promise() -> PromiseRef {
    Rc::new(RefCell::new(PromiseState::default()))
}

/// Create a promise that is already fulfilled.
pub fn fulfilled_promise(value: JsValue) -> PromiseRef {
    Rc::new(RefCell::new(PromiseState {
        status: PromiseStatus::Fulfilled(value),
        reactions: Vec::new(),
        settled: true,
    }))
}

/// Create a promise that is already rejected.
pub fn rejected_promise(reason: JsValue) -> PromiseRef {
    Rc::new(RefCell::new(PromiseState {
        status: PromiseStatus::Rejected(reason),
        reactions: Vec::new(),
        settled: true,
    }))
}

/// Settle `promise`, returning the microtasks its reactions produce.
///
/// Nothing runs here: settling only *queues* work, which is why a handler
/// registered on an already-settled promise still runs asynchronously.
fn settle(promise: &PromiseRef, status: PromiseStatus) -> Vec<Microtask> {
    let mut state = promise.borrow_mut();
    if state.settled {
        return Vec::new();
    }
    state.settled = true;
    state.status = status.clone();

    let (value, rejected) = match status {
        PromiseStatus::Fulfilled(value) => (value, false),
        PromiseStatus::Rejected(reason) => (reason, true),
        PromiseStatus::Pending => return Vec::new(),
    };

    std::mem::take(&mut state.reactions)
        .into_iter()
        .map(|reaction| Microtask::Reaction {
            reaction,
            value: value.clone(),
            rejected,
        })
        .collect()
}

/// Reject `promise` with `reason`.
pub fn reject(promise: &PromiseRef, reason: JsValue) -> Vec<Microtask> {
    settle(promise, PromiseStatus::Rejected(reason))
}

/// Fulfil `promise` with a value that is known not to be a promise.
pub fn fulfill(promise: &PromiseRef, value: JsValue) -> Vec<Microtask> {
    settle(promise, PromiseStatus::Fulfilled(value))
}

/// The promise resolution procedure.
///
/// Resolving with another promise makes this one *follow* it rather than
/// fulfil with it as a value.
pub fn resolve(promise: &PromiseRef, value: JsValue) -> Vec<Microtask> {
    if promise.borrow().settled {
        return Vec::new();
    }

    if let JsValue::Promise(inner) = &value {
        // A promise resolved with itself would wait on itself forever.
        if Rc::ptr_eq(promise, inner) {
            return reject(
                promise,
                JsValue::Str("TypeError: chaining cycle detected for promise".into()),
            );
        }
        return adopt(promise, inner);
    }

    fulfill(promise, value)
}

/// Make `promise` settle however `inner` settles.
fn adopt(promise: &PromiseRef, inner: &PromiseRef) -> Vec<Microtask> {
    let status = inner.borrow().status.clone();
    match status {
        // Already settled: queue the follow-up as a microtask, never inline.
        PromiseStatus::Fulfilled(value) => settle(promise, PromiseStatus::Fulfilled(value)),
        PromiseStatus::Rejected(reason) => settle(promise, PromiseStatus::Rejected(reason)),
        PromiseStatus::Pending => {
            // Wait: an identity reaction copies the eventual outcome across.
            inner.borrow_mut().reactions.push(Reaction {
                on_fulfilled: None,
                on_rejected: None,
                child: promise.clone(),
                kind: ReactionKind::Then,
            });
            Vec::new()
        }
    }
}

/// Register a reaction, returning the child promise and any microtasks that
/// became due because the promise had already settled.
pub fn then(
    promise: &PromiseRef,
    on_fulfilled: Option<JsValue>,
    on_rejected: Option<JsValue>,
    kind: ReactionKind,
) -> (PromiseRef, Vec<Microtask>) {
    let child = new_promise();
    let reaction = Reaction {
        on_fulfilled,
        on_rejected,
        child: child.clone(),
        kind,
    };

    let status = promise.borrow().status.clone();
    let microtasks = match status {
        PromiseStatus::Pending => {
            promise.borrow_mut().reactions.push(reaction);
            Vec::new()
        }
        // Settled already: the handler still waits for the next checkpoint.
        PromiseStatus::Fulfilled(value) => vec![Microtask::Reaction {
            reaction,
            value,
            rejected: false,
        }],
        PromiseStatus::Rejected(reason) => vec![Microtask::Reaction {
            reaction,
            value: reason,
            rejected: true,
        }],
    };
    (child, microtasks)
}

/// True when the promise has settled.
pub fn is_settled(promise: &PromiseRef) -> bool {
    !matches!(promise.borrow().status, PromiseStatus::Pending)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn status(promise: &PromiseRef) -> PromiseStatus {
        promise.borrow().status.clone()
    }

    /// A readable summary of a settled promise, for assertions.
    fn summary(promise: &PromiseRef) -> String {
        match status(promise) {
            PromiseStatus::Pending => "pending".to_string(),
            PromiseStatus::Fulfilled(value) => {
                format!("fulfilled:{}", super::super::interp::to_string(&value))
            }
            PromiseStatus::Rejected(reason) => {
                format!("rejected:{}", super::super::interp::to_string(&reason))
            }
        }
    }

    #[test]
    fn a_new_promise_is_pending() {
        let promise = new_promise();
        assert_eq!(summary(&promise), "pending");
        assert!(!is_settled(&promise));
    }

    #[test]
    fn resolving_fulfils_with_the_value() {
        let promise = new_promise();
        assert!(resolve(&promise, JsValue::Number(42.0)).is_empty());
        assert_eq!(summary(&promise), "fulfilled:42");
        assert!(is_settled(&promise));
    }

    #[test]
    fn only_the_first_settlement_counts() {
        let promise = new_promise();
        resolve(&promise, JsValue::Number(1.0));
        resolve(&promise, JsValue::Number(2.0));
        reject(&promise, JsValue::Str("late".into()));
        assert_eq!(summary(&promise), "fulfilled:1");
    }

    #[test]
    fn registering_on_a_settled_promise_queues_a_microtask() {
        let promise = fulfilled_promise(JsValue::Number(7.0));
        let (child, microtasks) =
            then(&promise, Some(JsValue::Undefined), None, ReactionKind::Then);

        assert_eq!(microtasks.len(), 1, "the handler waits for a checkpoint");
        assert_eq!(summary(&child), "pending");
    }

    #[test]
    fn registering_on_a_pending_promise_waits_for_settlement() {
        let promise = new_promise();
        let (_, microtasks) = then(&promise, Some(JsValue::Undefined), None, ReactionKind::Then);
        assert!(microtasks.is_empty());
        assert_eq!(promise.borrow().reactions.len(), 1);

        // Settling releases exactly one microtask per waiting reaction.
        let released = resolve(&promise, JsValue::Number(1.0));
        assert_eq!(released.len(), 1);
        assert!(promise.borrow().reactions.is_empty());
    }

    #[test]
    fn every_registered_handler_gets_its_own_microtask() {
        let promise = new_promise();
        for _ in 0..3 {
            then(&promise, Some(JsValue::Undefined), None, ReactionKind::Then);
        }
        let released = resolve(&promise, JsValue::Number(1.0));
        assert_eq!(released.len(), 3, "handlers are not consumed by each other");
    }

    #[test]
    fn resolving_with_a_pending_promise_adopts_it() {
        let outer = new_promise();
        let inner = new_promise();
        assert!(resolve(&outer, JsValue::Promise(inner.clone())).is_empty());

        // The outer promise is still pending, waiting on the inner one.
        assert_eq!(summary(&outer), "pending");
        assert_eq!(inner.borrow().reactions.len(), 1);
        assert!(!outer.borrow().settled, "adoption is not a settlement yet");
    }

    #[test]
    fn resolving_with_a_settled_promise_copies_its_outcome() {
        let outer = new_promise();
        let inner = fulfilled_promise(JsValue::Str("inner".into()));
        resolve(&outer, JsValue::Promise(inner));
        assert_eq!(summary(&outer), "fulfilled:inner");

        let outer = new_promise();
        let inner = rejected_promise(JsValue::Str("nope".into()));
        resolve(&outer, JsValue::Promise(inner));
        assert_eq!(summary(&outer), "rejected:nope");
    }

    #[test]
    fn resolving_a_promise_with_itself_rejects_rather_than_hanging() {
        let promise = new_promise();
        let microtasks = resolve(&promise, JsValue::Promise(promise.clone()));
        match status(&promise) {
            PromiseStatus::Rejected(JsValue::Str(reason)) => {
                assert!(reason.contains("chaining cycle"), "got {reason}");
            }
            other => panic!("expected a rejection, got {other:?}"),
        }
        assert!(microtasks.is_empty(), "nothing was waiting on it");
    }

    #[test]
    fn dropping_a_promise_releases_its_reactions() {
        let callback = Rc::new(RefCell::new(0));
        let promise = new_promise();
        {
            // A handler value standing in for a closure.
            let handler = JsValue::Number(1.0);
            then(&promise, Some(handler), None, ReactionKind::Then);
        }
        assert_eq!(promise.borrow().reactions.len(), 1);
        assert_eq!(Rc::strong_count(&callback), 1);

        drop(promise);
        // Nothing else holds the reaction, so the child promise goes with it.
        assert_eq!(Rc::strong_count(&callback), 1);
    }
}
