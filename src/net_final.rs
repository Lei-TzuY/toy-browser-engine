// ============================================================
//  net_final.rs — public network facade with completion accounting
// ============================================================
//
// `net_base` retains the protocol/loaders and raw worker implementation. This
// final facade strengthens one lifecycle invariant at the public boundary:
// once `start(id, ...)` returns, `is_busy()` stays true until that id is either
// observed as a completion or explicitly cancelled. Worker-thread liveness is
// therefore no longer used as a proxy for completion visibility.

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

pub use crate::net_base::{
    fetch, http, mime_from_path, static_fetch, url, url_from_argument, DefaultLoader, FileLoader,
    FetchCompletion, FetchError, FetchId, FetchRegistry, FetchRequest, FetchResponse, HeaderMap,
    HttpConfig, HttpLoader, LoadError, LocalNetwork, ManualNetwork, MemoryLoader, Method,
    NetworkBackend, OfflineNetwork, Origin, Resource, ResourceLoader, Url, UrlError,
};

/// Adds request-id accounting around a backend.
///
/// A completion is the only normal operation that retires an outstanding id.
/// This avoids a subtle race in threaded backends where a worker can be
/// observed as finished immediately before its channel send becomes visible to
/// a non-blocking receiver. `is_busy()` is about unfinished *requests*, not
/// worker handles, so it must not return false during that window.
struct CompletionTracked<N> {
    inner: N,
    ready: RefCell<Vec<FetchCompletion>>,
    outstanding: RefCell<HashSet<FetchId>>,
}

impl<N: NetworkBackend> CompletionTracked<N> {
    fn new(inner: N) -> Self {
        Self {
            inner,
            ready: RefCell::new(Vec::new()),
            outstanding: RefCell::new(HashSet::new()),
        }
    }

    fn start(&self, id: FetchId, request: FetchRequest) {
        self.outstanding.borrow_mut().insert(id);
        self.inner.start(id, request);
    }

    /// Drain the inner backend and move only still-outstanding completions into
    /// the public ready queue. A late answer for a cancelled id is discarded.
    fn harvest(&self) {
        let completions = self.inner.poll();
        if completions.is_empty() {
            return;
        }

        let mut outstanding = self.outstanding.borrow_mut();
        let mut ready = self.ready.borrow_mut();
        for completion in completions {
            if outstanding.remove(&completion.id) {
                ready.push(completion);
            }
        }
    }

    fn poll(&self) -> Vec<FetchCompletion> {
        self.harvest();
        std::mem::take(&mut *self.ready.borrow_mut())
    }

    fn cancel(&self, id: FetchId) {
        self.outstanding.borrow_mut().remove(&id);
        self.ready
            .borrow_mut()
            .retain(|completion| completion.id != id);
        self.inner.cancel(id);
    }

    fn is_busy(&self) -> bool {
        self.harvest();
        !self.ready.borrow().is_empty() || !self.outstanding.borrow().is_empty()
    }

    fn wait(&self, timeout: Duration) -> bool {
        self.harvest();
        if !self.ready.borrow().is_empty() {
            return true;
        }
        if self.outstanding.borrow().is_empty() {
            return false;
        }

        let arrived = self.inner.wait(timeout);
        self.harvest();
        arrived || !self.ready.borrow().is_empty()
    }
}

/// Threaded backend whose busy state is tied to request completion rather than
/// the worker thread's instantaneous liveness.
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

/// Default routing backend with the same request-completion accounting.
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
    use std::cell::Cell;

    /// Models the exact bad ordering from the real threaded backend: the first
    /// non-blocking poll sees nothing even though the backend itself already
    /// reports idle; the completion becomes visible on the next poll.
    struct CompletionAfterFalseIdle {
        polls: Cell<u8>,
    }

    impl NetworkBackend for CompletionAfterFalseIdle {
        fn start(&self, _id: FetchId, _request: FetchRequest) {}

        fn poll(&self) -> Vec<FetchCompletion> {
            let poll = self.polls.get();
            self.polls.set(poll + 1);
            if poll == 1 {
                vec![FetchCompletion {
                    id: 7,
                    result: Err(FetchError::Aborted),
                }]
            } else {
                Vec::new()
            }
        }

        fn is_busy(&self) -> bool {
            false
        }
    }

    fn request() -> FetchRequest {
        FetchRequest::get(Url::parse("demo:///resource").unwrap())
    }

    #[test]
    fn outstanding_id_keeps_false_idle_backend_busy_until_completion_is_observed() {
        let network = CompletionTracked::new(CompletionAfterFalseIdle {
            polls: Cell::new(0),
        });
        network.start(7, request());

        // First harvest sees no channel item. The request id itself is the
        // authoritative proof that work is still outstanding.
        assert!(network.is_busy());

        let completions = network.poll();
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].id, 7);
        assert!(!network.is_busy());
    }

    #[test]
    fn cancellation_retires_the_id_and_discards_a_late_completion() {
        let network = CompletionTracked::new(CompletionAfterFalseIdle {
            polls: Cell::new(0),
        });
        network.start(7, request());
        network.cancel(7);
        assert!(!network.is_busy());
        assert!(network.poll().is_empty());
    }

    #[test]
    fn immediate_threaded_completions_never_create_a_false_idle_window() {
        let mut loader = MemoryLoader::new();
        loader.insert("demo:///resource", "ok");
        let network = ThreadedNetwork::new(Arc::new(loader));

        // Fast in-memory loads make the worker/send/exit ordering extremely
        // tight, which amplifies the race seen on macOS ARM without relying on
        // sleeps or wall-clock deadlines. Reuse one backend so channel and
        // worker bookkeeping are exercised repeatedly too.
        for id in 1..=256 {
            network.start(id, request());
            loop {
                if let Some(completion) = network.poll().into_iter().next() {
                    assert_eq!(completion.id, id);
                    assert_eq!(completion.result.unwrap().text(), "ok");
                    break;
                }
                assert!(
                    network.is_busy(),
                    "request {id} became falsely idle before its completion was observed"
                );
                std::thread::yield_now();
            }
            assert!(!network.is_busy());
        }
    }
}
