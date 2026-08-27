// ============================================================
//  net_final.rs — public network facade with completion accounting
// ============================================================
//
// `net_base` retains the protocol/loaders and raw worker implementation. This
// final facade strengthens two lifecycle invariants at the public boundary:
// a started request stays busy until its completion is observed or cancelled,
// and every start receives a fresh internal wire id so a stale answer from an
// older generation can never be mistaken for a reused public FetchId.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[path = "fetch_final.rs"]
pub mod fetch;

pub use crate::net_base::{
    http, mime_from_path, static_fetch, url, url_from_argument, DefaultLoader, FileLoader,
    FetchCompletion, FetchError, FetchId, FetchRequest, FetchResponse, HeaderMap, HttpConfig,
    HttpLoader, LoadError, LocalNetwork, ManualNetwork, MemoryLoader, Method, NetworkBackend,
    OfflineNetwork, Origin, Resource, ResourceLoader, Url, UrlError,
};
pub use fetch::FetchRegistry;

/// Adds public-id lifecycle accounting and internal request generations around
/// a backend.
///
/// The raw backends key completion and cancellation only by `FetchId`. Passing
/// a public id through unchanged therefore makes reuse unsafe: a late answer or
/// cancellation marker from an older request can be confused with a newer
/// request carrying the same id. `CompletionTracked` instead gives every start
/// a monotonically increasing internal wire id and translates only the active
/// generation back to the caller's public id.
struct CompletionTracked<N> {
    inner: N,
    ready: RefCell<Vec<FetchCompletion>>,
    public_to_wire: RefCell<HashMap<FetchId, FetchId>>,
    wire_to_public: RefCell<HashMap<FetchId, FetchId>>,
    /// Internal ids deliberately never wrap. Exhausting 2^64 generations in a
    /// single backend lifetime is treated as a fatal invariant violation rather
    /// than silently reusing a token that a stale worker may still carry.
    next_wire_id: Cell<FetchId>,
}

impl<N: NetworkBackend> CompletionTracked<N> {
    fn new(inner: N) -> Self {
        Self {
            inner,
            ready: RefCell::new(Vec::new()),
            public_to_wire: RefCell::new(HashMap::new()),
            wire_to_public: RefCell::new(HashMap::new()),
            next_wire_id: Cell::new(1),
        }
    }

    fn allocate_wire_id(&self) -> FetchId {
        let wire = self.next_wire_id.get();
        let next = wire
            .checked_add(1)
            .expect("network request generation id space exhausted");
        self.next_wire_id.set(next);
        wire
    }

    fn start(&self, public_id: FetchId, request: FetchRequest) {
        // A duplicate public id is treated as a new generation. Retire the old
        // generation first so a stale completion cannot settle the replacement.
        if let Some(old_wire) = self.public_to_wire.borrow_mut().remove(&public_id) {
            self.wire_to_public.borrow_mut().remove(&old_wire);
            self.inner.cancel(old_wire);
        }
        self.ready
            .borrow_mut()
            .retain(|completion| completion.id != public_id);

        let wire_id = self.allocate_wire_id();
        self.public_to_wire
            .borrow_mut()
            .insert(public_id, wire_id);
        self.wire_to_public
            .borrow_mut()
            .insert(wire_id, public_id);
        self.inner.start(wire_id, request);
    }

    /// Drain the inner backend and translate only the currently active wire
    /// generation. A late answer for a cancelled/superseded generation has no
    /// mapping and is discarded even if its public id has since been reused.
    fn harvest(&self) {
        let completions = self.inner.poll();
        if completions.is_empty() {
            return;
        }

        let mut wire_to_public = self.wire_to_public.borrow_mut();
        let mut public_to_wire = self.public_to_wire.borrow_mut();
        let mut ready = self.ready.borrow_mut();
        for mut completion in completions {
            let wire_id = completion.id;
            let Some(public_id) = wire_to_public.remove(&wire_id) else {
                continue;
            };
            if public_to_wire.get(&public_id).copied() != Some(wire_id) {
                continue;
            }
            public_to_wire.remove(&public_id);
            completion.id = public_id;
            ready.push(completion);
        }
    }

    fn poll(&self) -> Vec<FetchCompletion> {
        self.harvest();
        std::mem::take(&mut *self.ready.borrow_mut())
    }

    fn cancel(&self, public_id: FetchId) {
        self.ready
            .borrow_mut()
            .retain(|completion| completion.id != public_id);
        let wire_id = self.public_to_wire.borrow_mut().remove(&public_id);
        if let Some(wire_id) = wire_id {
            self.wire_to_public.borrow_mut().remove(&wire_id);
            self.inner.cancel(wire_id);
        }
    }

    fn is_busy(&self) -> bool {
        self.harvest();
        !self.ready.borrow().is_empty() || !self.public_to_wire.borrow().is_empty()
    }

    fn wait(&self, timeout: Duration) -> bool {
        self.harvest();
        if !self.ready.borrow().is_empty() {
            return true;
        }
        if self.public_to_wire.borrow().is_empty() || timeout.is_zero() {
            return false;
        }

        // A raw backend wake-up only means that some wire completion arrived.
        // It may belong to a cancelled/superseded generation and disappear in
        // `harvest()`. Keep waiting for the remaining budget while public work
        // is still outstanding instead of leaking that stale wake-up through
        // the public readiness contract.
        let started = Instant::now();
        let mut remaining = timeout;
        loop {
            if !self.inner.wait(remaining) {
                self.harvest();
                return !self.ready.borrow().is_empty();
            }

            self.harvest();
            if !self.ready.borrow().is_empty() {
                return true;
            }
            if self.public_to_wire.borrow().is_empty() {
                return false;
            }

            let elapsed = started.elapsed();
            let Some(next_remaining) = timeout.checked_sub(elapsed) else {
                return false;
            };
            if next_remaining.is_zero() {
                return false;
            }
            remaining = next_remaining;
        }
    }
}

/// Threaded backend whose busy state and completion identity are tied to the
/// active request generation rather than worker-thread liveness.
pub struct ThreadedNetwork {
    backend: CompletionTracked<crate::net_base::ThreadedNetwork>,
}

impl ThreadedNetwork {
    pub fn new(loader: Arc<dyn ResourceLoader>) -> Self {
        Self {
            backend: CompletionTracked::new(crate::net_base::ThreadedNetwork::new(loader)),
        }
    }
}

impl NetworkBackend for ThreadedNetwork {
    fn start(&self, id: FetchId, request: FetchRequest) {
        self.backend.start(id, request);
    }

    fn poll(&self) -> Vec<FetchCompletion> {
        self.backend.poll()
    }

    fn cancel(&self, id: FetchId) {
        self.backend.cancel(id);
    }

    fn is_busy(&self) -> bool {
        self.backend.is_busy()
    }

    fn wait(&self, timeout: Duration) -> bool {
        self.backend.wait(timeout)
    }
}

/// Default routing backend with the same request-generation accounting.
pub struct DefaultNetwork {
    backend: CompletionTracked<crate::net_base::DefaultNetwork>,
}

impl DefaultNetwork {
    pub fn new(loader: Arc<dyn ResourceLoader>) -> Self {
        Self {
            backend: CompletionTracked::new(crate::net_base::DefaultNetwork::new(loader)),
        }
    }
}

impl NetworkBackend for DefaultNetwork {
    fn start(&self, id: FetchId, request: FetchRequest) {
        self.backend.start(id, request);
    }

    fn poll(&self) -> Vec<FetchCompletion> {
        self.backend.poll()
    }

    fn cancel(&self, id: FetchId) {
        self.backend.cancel(id);
    }

    fn is_busy(&self) -> bool {
        self.backend.is_busy()
    }

    fn wait(&self, timeout: Duration) -> bool {
        self.backend.wait(timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct DelayedNetwork {
        started: RefCell<Vec<FetchId>>,
        cancelled: RefCell<Vec<FetchId>>,
        ready: RefCell<Vec<FetchCompletion>>,
        wait_ready: RefCell<Vec<Vec<FetchCompletion>>>,
        wait_calls: Cell<usize>,
    }

    impl DelayedNetwork {
        fn text_completion(wire_id: FetchId, text: &str) -> FetchCompletion {
            FetchCompletion {
                id: wire_id,
                result: Ok(FetchResponse::synthetic(
                    Url::parse("demo:///resource").unwrap(),
                    200,
                    Some("text/plain"),
                    text.as_bytes().to_vec(),
                )),
            }
        }

        fn complete_text(&self, wire_id: FetchId, text: &str) {
            self.ready
                .borrow_mut()
                .push(Self::text_completion(wire_id, text));
        }

        fn wake_with_text(&self, wire_id: FetchId, text: &str) {
            self.wait_ready
                .borrow_mut()
                .push(vec![Self::text_completion(wire_id, text)]);
        }
    }

    impl NetworkBackend for DelayedNetwork {
        fn start(&self, id: FetchId, _request: FetchRequest) {
            self.started.borrow_mut().push(id);
        }

        fn poll(&self) -> Vec<FetchCompletion> {
            std::mem::take(&mut *self.ready.borrow_mut())
        }

        fn cancel(&self, id: FetchId) {
            self.cancelled.borrow_mut().push(id);
        }

        // Deliberately false: the wrapper's own request map, not raw liveness,
        // is the authority on whether public work is still outstanding.
        fn is_busy(&self) -> bool {
            false
        }

        fn wait(&self, _timeout: Duration) -> bool {
            self.wait_calls.set(self.wait_calls.get() + 1);
            if self.wait_ready.borrow().is_empty() {
                return false;
            }
            let batch = self.wait_ready.borrow_mut().remove(0);
            self.ready.borrow_mut().extend(batch);
            true
        }
    }

    fn request() -> FetchRequest {
        FetchRequest::get(Url::parse("demo:///resource").unwrap())
    }

    #[test]
    fn active_request_stays_busy_even_if_inner_backend_reports_idle() {
        let network = CompletionTracked::new(DelayedNetwork::default());
        network.start(7, request());
        let wire = network.public_to_wire.borrow()[&7];

        assert!(network.is_busy());
        network.inner.complete_text(wire, "ok");

        let completions = network.poll();
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].id, 7);
        assert_eq!(completions[0].result.as_ref().unwrap().body, b"ok");
        assert!(!network.is_busy());
    }

    #[test]
    fn cancellation_retires_the_generation_and_discards_a_late_completion() {
        let network = CompletionTracked::new(DelayedNetwork::default());
        network.start(7, request());
        let wire = network.public_to_wire.borrow()[&7];
        network.cancel(7);

        assert_eq!(&*network.inner.cancelled.borrow(), &[wire]);
        network.inner.complete_text(wire, "late");
        assert!(network.poll().is_empty());
        assert!(!network.is_busy());
    }

    #[test]
    fn reused_public_id_is_isolated_from_the_cancelled_generation() {
        let network = CompletionTracked::new(DelayedNetwork::default());
        network.start(7, request());
        let old_wire = network.public_to_wire.borrow()[&7];
        network.cancel(7);

        network.start(7, request());
        let new_wire = network.public_to_wire.borrow()[&7];
        assert_ne!(old_wire, new_wire);

        network.inner.complete_text(old_wire, "old");
        assert!(network.poll().is_empty());
        assert!(network.is_busy(), "the new generation is still outstanding");

        network.inner.complete_text(new_wire, "new");
        let completions = network.poll();
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].id, 7);
        assert_eq!(completions[0].result.as_ref().unwrap().body, b"new");
        assert!(!network.is_busy());
    }

    #[test]
    fn duplicate_active_public_id_supersedes_the_old_generation() {
        let network = CompletionTracked::new(DelayedNetwork::default());
        network.start(9, request());
        let old_wire = network.public_to_wire.borrow()[&9];
        network.start(9, request());
        let new_wire = network.public_to_wire.borrow()[&9];

        assert_ne!(old_wire, new_wire);
        assert_eq!(&*network.inner.cancelled.borrow(), &[old_wire]);

        network.inner.complete_text(old_wire, "old");
        network.inner.complete_text(new_wire, "new");
        let completions = network.poll();
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].id, 9);
        assert_eq!(completions[0].result.as_ref().unwrap().body, b"new");
    }

    #[test]
    fn wait_ignores_stale_generation_wakeup_and_keeps_waiting_for_active_one() {
        let network = CompletionTracked::new(DelayedNetwork::default());
        network.start(7, request());
        let old_wire = network.public_to_wire.borrow()[&7];
        network.cancel(7);
        network.start(7, request());
        let new_wire = network.public_to_wire.borrow()[&7];

        network.inner.wake_with_text(old_wire, "stale");
        network.inner.wake_with_text(new_wire, "fresh");

        assert!(network.wait(Duration::from_secs(1)));
        assert_eq!(network.inner.wait_calls.get(), 2);

        let completions = network.poll();
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].id, 7);
        assert_eq!(completions[0].result.as_ref().unwrap().body, b"fresh");
        assert!(!network.is_busy());
    }

    #[test]
    fn wait_returns_false_after_stale_wakeup_when_active_generation_has_no_answer() {
        let network = CompletionTracked::new(DelayedNetwork::default());
        network.start(7, request());
        let old_wire = network.public_to_wire.borrow()[&7];
        network.cancel(7);
        network.start(7, request());

        network.inner.wake_with_text(old_wire, "stale");

        assert!(!network.wait(Duration::from_secs(1)));
        assert_eq!(network.inner.wait_calls.get(), 2);
        assert!(network.is_busy());
    }

    #[test]
    fn immediate_threaded_completions_never_create_a_false_idle_window() {
        let mut loader = MemoryLoader::new();
        loader.insert("demo:///resource", "ok");
        let network = ThreadedNetwork::new(Arc::new(loader));

        // Fast in-memory loads make worker/send/exit ordering extremely tight.
        // Reuse public ids across many generations as well, so the stress test
        // covers both completion visibility and public-to-wire translation.
        for generation in 1..=256 {
            let public_id = (generation % 5) + 1;
            network.start(public_id, request());
            loop {
                if let Some(completion) = network.poll().into_iter().next() {
                    assert_eq!(completion.id, public_id);
                    assert_eq!(completion.result.unwrap().body, b"ok");
                    break;
                }
                assert!(
                    network.is_busy(),
                    "generation {generation} became falsely idle before completion"
                );
                std::thread::yield_now();
            }
            assert!(!network.is_busy());
        }
    }
}
