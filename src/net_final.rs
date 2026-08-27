// ============================================================
//  net_final.rs — public network facade with completion accounting
// ============================================================
//
// `net_base` retains the protocol/loaders and raw implementation. This final
// facade owns the asynchronous worker boundary used publicly: started requests
// stay busy until observed or cancelled, every start gets a fresh internal wire
// generation, stale generations are invisible, and worker threads are detached
// so their JoinHandles cannot accumulate in a long-lived browser session.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{channel, Receiver, Sender};
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

/// Minimal threaded wire backend used behind [`CompletionTracked`].
///
/// It deliberately does not retain `JoinHandle`s. The completion/generation
/// maps in the facade are the authoritative liveness state, so keeping one
/// handle per request only creates a cleanup obligation after cancellation.
/// Dropping the handle detaches the worker while the channel still owns the
/// result path.
struct DetachedThreadedBackend {
    loader: Arc<dyn ResourceLoader>,
    sender: Sender<FetchCompletion>,
    receiver: Receiver<FetchCompletion>,
    ready: RefCell<Vec<FetchCompletion>>,
    outstanding: RefCell<HashSet<FetchId>>,
    cancelled: RefCell<HashSet<FetchId>>,
}

impl DetachedThreadedBackend {
    fn new(loader: Arc<dyn ResourceLoader>) -> Self {
        let (sender, receiver) = channel();
        Self {
            loader,
            sender,
            receiver,
            ready: RefCell::new(Vec::new()),
            outstanding: RefCell::new(HashSet::new()),
            cancelled: RefCell::new(HashSet::new()),
        }
    }
}

impl NetworkBackend for DetachedThreadedBackend {
    fn start(&self, id: FetchId, request: FetchRequest) {
        self.outstanding.borrow_mut().insert(id);
        let loader = self.loader.clone();
        let sender = self.sender.clone();
        let request_url = request.url.to_string();

        // Intentionally discard the JoinHandle: completion visibility is
        // tracked by request id, not thread liveness. Contain a loader panic so
        // every started generation still produces a terminal completion.
        let _ = std::thread::spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(|| loader.fetch(&request))).unwrap_or_else(
                |_| {
                    Err(FetchError::Io(format!(
                        "network worker panicked while fetching {request_url}"
                    )))
                },
            );
            let _ = sender.send(FetchCompletion { id, result });
        });
    }

    fn poll(&self) -> Vec<FetchCompletion> {
        let mut arrived = std::mem::take(&mut *self.ready.borrow_mut());
        arrived.extend(self.receiver.try_iter());

        let mut outstanding = self.outstanding.borrow_mut();
        let mut cancelled = self.cancelled.borrow_mut();
        arrived
            .into_iter()
            .filter(|completion| {
                let active = outstanding.remove(&completion.id);
                let was_cancelled = cancelled.remove(&completion.id);
                active && !was_cancelled
            })
            .collect()
    }

    fn cancel(&self, id: FetchId) {
        if self.outstanding.borrow_mut().remove(&id) {
            // The detached worker may already be inside the loader. Its answer
            // is allowed to arrive, but this marker makes that answer invisible.
            self.cancelled.borrow_mut().insert(id);
        }
        self.ready
            .borrow_mut()
            .retain(|completion| completion.id != id);
    }

    fn is_busy(&self) -> bool {
        !self.outstanding.borrow().is_empty() || !self.ready.borrow().is_empty()
    }

    fn wait(&self, timeout: Duration) -> bool {
        if !self.ready.borrow().is_empty() {
            return true;
        }
        if self.outstanding.borrow().is_empty() {
            return false;
        }
        match self.receiver.recv_timeout(timeout) {
            Ok(completion) => {
                self.ready.borrow_mut().push(completion);
                true
            }
            Err(_) => false,
        }
    }
}

/// Scheme router used by the public default network. Local resources retain
/// their synchronous backend; only HTTP(S) work crosses the detached worker
/// boundary.
struct RoutedBackend {
    local: crate::net_base::LocalNetwork,
    threaded: DetachedThreadedBackend,
}

impl RoutedBackend {
    fn new(loader: Arc<dyn ResourceLoader>) -> Self {
        Self {
            local: crate::net_base::LocalNetwork::new(loader.clone()),
            threaded: DetachedThreadedBackend::new(loader),
        }
    }
}

impl NetworkBackend for RoutedBackend {
    fn start(&self, id: FetchId, request: FetchRequest) {
        match request.url.scheme() {
            "http" | "https" => self.threaded.start(id, request),
            _ => self.local.start(id, request),
        }
    }

    fn poll(&self) -> Vec<FetchCompletion> {
        let mut completions = self.local.poll();
        completions.extend(self.threaded.poll());
        completions
    }

    fn cancel(&self, id: FetchId) {
        self.local.cancel(id);
        self.threaded.cancel(id);
    }

    fn is_busy(&self) -> bool {
        self.local.is_busy() || self.threaded.is_busy()
    }

    fn wait(&self, timeout: Duration) -> bool {
        // Local work completes during start and is ready immediately. Avoid
        // blocking on the socket channel when that local completion is waiting.
        self.local.is_busy() || self.threaded.wait(timeout)
    }
}

/// Adds public-id lifecycle accounting and internal request generations around
/// a backend.
///
/// The wire backend keys completion and cancellation only by `FetchId`. Passing
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
        if self.public_to_wire.borrow().is_empty() {
            return false;
        }

        // A wire backend can wake for a completion from a generation that was
        // cancelled or superseded after the worker started. Such a wake is not
        // observable at this public boundary, and it must not make `wait`
        // return early while an active generation is still owed. Keep waiting
        // with the remaining portion of the caller's original timeout budget.
        let started = Instant::now();
        let mut remaining = timeout;
        loop {
            let woke = self.inner.wait(remaining);
            self.harvest();

            if !self.ready.borrow().is_empty() {
                return true;
            }
            if self.public_to_wire.borrow().is_empty() {
                return false;
            }
            if !woke {
                return false;
            }

            let elapsed = started.elapsed();
            if elapsed >= timeout {
                return false;
            }
            remaining = timeout.saturating_sub(elapsed);
        }
    }
}

/// Threaded backend whose busy state and completion identity are tied to the
/// active request generation rather than worker-thread liveness.
pub struct ThreadedNetwork {
    backend: CompletionTracked<DetachedThreadedBackend>,
}

impl ThreadedNetwork {
    pub fn new(loader: Arc<dyn ResourceLoader>) -> Self {
        Self {
            backend: CompletionTracked::new(DetachedThreadedBackend::new(loader)),
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

/// Default routing backend with the same request-generation accounting and
/// detached HTTP workers.
pub struct DefaultNetwork {
    backend: CompletionTracked<RoutedBackend>,
}

impl DefaultNetwork {
    pub fn new(loader: Arc<dyn ResourceLoader>) -> Self {
        Self {
            backend: CompletionTracked::new(RoutedBackend::new(loader)),
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
    }

    impl DelayedNetwork {
        fn complete_text(&self, wire_id: FetchId, text: &str) {
            self.ready.borrow_mut().push(text_completion(wire_id, text));
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
    }

    /// Simulates the most important wait edge after request-generation
    /// isolation: the raw backend first wakes for a stale cancelled generation,
    /// then for the active replacement. Cancelling is deliberately advisory so
    /// the stale completion can still race in, just like a worker already on
    /// the wire.
    #[derive(Default)]
    struct StaleThenActiveWaitNetwork {
        started: RefCell<Vec<FetchId>>,
        ready: RefCell<Vec<FetchCompletion>>,
        waits: Cell<u8>,
    }

    impl NetworkBackend for StaleThenActiveWaitNetwork {
        fn start(&self, id: FetchId, _request: FetchRequest) {
            self.started.borrow_mut().push(id);
        }

        fn poll(&self) -> Vec<FetchCompletion> {
            std::mem::take(&mut *self.ready.borrow_mut())
        }

        fn cancel(&self, _id: FetchId) {
            // Advisory only: model a request that is already executing.
        }

        fn wait(&self, _timeout: Duration) -> bool {
            let call = self.waits.get();
            self.waits.set(call + 1);
            let started = self.started.borrow();
            let wire_id = match call {
                0 => started.first().copied(),
                1 => started.get(1).copied(),
                _ => None,
            };
            let Some(wire_id) = wire_id else {
                return false;
            };
            let text = if call == 0 { "stale" } else { "active" };
            self.ready.borrow_mut().push(text_completion(wire_id, text));
            true
        }
    }

    struct PanickingLoader;

    impl ResourceLoader for PanickingLoader {
        fn load(&self, _url: &Url) -> Result<Resource, LoadError> {
            panic!("intentional loader panic")
        }

        fn fetch(&self, _request: &FetchRequest) -> Result<FetchResponse, FetchError> {
            panic!("intentional loader panic")
        }
    }

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
    fn wait_ignores_stale_wakeup_and_keeps_waiting_for_active_generation() {
        let network = CompletionTracked::new(StaleThenActiveWaitNetwork::default());
        network.start(7, request());
        network.cancel(7);
        network.start(7, request());

        assert!(network.wait(Duration::from_secs(1)));
        assert_eq!(network.inner.waits.get(), 2, "stale wake must be swallowed");

        let completions = network.poll();
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].id, 7);
        assert_eq!(completions[0].result.as_ref().unwrap().body, b"active");
        assert!(!network.is_busy());
    }

    #[test]
    fn out_of_order_completions_preserve_public_identity_and_arrival_order() {
        let network = CompletionTracked::new(DelayedNetwork::default());
        network.start(1, request());
        network.start(2, request());
        network.start(3, request());
        let one = network.public_to_wire.borrow()[&1];
        let two = network.public_to_wire.borrow()[&2];
        let three = network.public_to_wire.borrow()[&3];

        network.inner.complete_text(three, "three");
        network.inner.complete_text(one, "one");
        network.inner.complete_text(two, "two");

        let completions = network.poll();
        assert_eq!(
            completions.iter().map(|completion| completion.id).collect::<Vec<_>>(),
            vec![3, 1, 2]
        );
        assert_eq!(
            completions
                .iter()
                .map(|completion| completion.result.as_ref().unwrap().body.clone())
                .collect::<Vec<_>>(),
            vec![b"three".to_vec(), b"one".to_vec(), b"two".to_vec()]
        );
        assert!(!network.is_busy());
    }

    #[test]
    fn cancel_after_harvest_but_before_poll_discards_ready_completion() {
        let network = CompletionTracked::new(DelayedNetwork::default());
        network.start(5, request());
        let wire = network.public_to_wire.borrow()[&5];
        network.inner.complete_text(wire, "ready");

        assert!(network.is_busy(), "is_busy harvests the ready completion");
        network.cancel(5);

        assert!(network.poll().is_empty());
        assert!(network.inner.cancelled.borrow().is_empty());
        assert!(!network.is_busy());
    }

    #[test]
    fn worker_panic_becomes_fetch_error_and_retires_generation() {
        let network = ThreadedNetwork::new(Arc::new(PanickingLoader));
        network.start(77, request());

        assert!(network.wait(Duration::from_secs(2)));
        let mut completions = network.poll();
        assert_eq!(completions.len(), 1);
        let completion = completions.pop().unwrap();
        assert_eq!(completion.id, 77);
        match completion.result {
            Err(FetchError::Io(message)) => {
                assert!(message.contains("network worker panicked"), "{message}")
            }
            other => panic!("expected contained worker panic, got {other:?}"),
        }
        assert!(!network.is_busy());
    }

    #[test]
    fn immediate_threaded_completions_never_create_a_false_idle_window() {
        let mut loader = MemoryLoader::new();
        loader.insert("demo:///resource", "ok");
        let network = ThreadedNetwork::new(Arc::new(loader));

        // Fast in-memory loads make worker/send ordering extremely tight.
        // Reuse public ids across many generations as well, so the stress test
        // covers completion visibility and public-to-wire translation without
        // relying on worker JoinHandle state.
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
