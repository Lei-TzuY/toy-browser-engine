// ============================================================
//  net/readiness.rs — completion-aware public network wrappers
// ============================================================
//
// A worker can send a completion and exit in the narrow window between a
// caller's `poll()` and `is_busy()`. The raw threaded backend owns the socket
// and worker lifecycle; this module strengthens only the public readiness
// contract so an already-produced answer cannot be mistaken for an idle
// network.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

use super::fetch_core::{FetchCompletion, FetchId, FetchRequest, NetworkBackend};
use super::{fetch_core, ResourceLoader};

/// Adds a small ready queue around a backend so a completion remains observable
/// across the `poll()` -> `is_busy()` boundary.
///
/// `is_busy()` drains once before asking the inner backend and, after observing
/// the inner backend idle, drains once more. A worker sends its completion
/// before it exits, so if it transitions from running to finished between those
/// observations the final poll must see that completion.
struct CompletionAware<N> {
    inner: N,
    ready: RefCell<Vec<FetchCompletion>>,
}

impl<N: NetworkBackend> CompletionAware<N> {
    fn new(inner: N) -> Self {
        Self {
            inner,
            ready: RefCell::new(Vec::new()),
        }
    }

    fn poll(&self) -> Vec<FetchCompletion> {
        let mut ready = std::mem::take(&mut *self.ready.borrow_mut());
        ready.extend(self.inner.poll());
        ready
    }

    fn cancel(&self, id: FetchId) {
        self.ready
            .borrow_mut()
            .retain(|completion| completion.id != id);
        self.inner.cancel(id);
    }

    fn is_busy(&self) -> bool {
        self.ready.borrow_mut().extend(self.inner.poll());
        if !self.ready.borrow().is_empty() {
            return true;
        }
        if self.inner.is_busy() {
            return true;
        }

        // The inner backend reported idle. If a worker sent and exited after
        // the first poll, worker exit necessarily happened after the send, so
        // one final drain closes that race without sleeps or guessed delays.
        self.ready.borrow_mut().extend(self.inner.poll());
        !self.ready.borrow().is_empty()
    }

    fn wait(&self, timeout: Duration) -> bool {
        if !self.ready.borrow().is_empty() {
            true
        } else {
            self.inner.wait(timeout)
        }
    }
}

/// Public threaded backend with race-free ready/busy observation.
pub struct ThreadedNetwork {
    backend: CompletionAware<fetch_core::ThreadedNetwork>,
}

impl ThreadedNetwork {
    pub fn new(loader: Arc<dyn ResourceLoader>) -> Self {
        Self {
            backend: CompletionAware::new(fetch_core::ThreadedNetwork::new(loader)),
        }
    }
}

impl NetworkBackend for ThreadedNetwork {
    fn start(&self, id: FetchId, request: FetchRequest) {
        self.backend.inner.start(id, request);
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

/// Public routing backend with the same completion visibility guarantee.
///
/// `fetch_core::DefaultNetwork` still owns local-vs-threaded routing. This
/// wrapper changes only readiness observation at the Browser-facing boundary.
pub struct DefaultNetwork {
    backend: CompletionAware<fetch_core::DefaultNetwork>,
}

impl DefaultNetwork {
    pub fn new(loader: Arc<dyn ResourceLoader>) -> Self {
        Self {
            backend: CompletionAware::new(fetch_core::DefaultNetwork::new(loader)),
        }
    }
}

impl NetworkBackend for DefaultNetwork {
    fn start(&self, id: FetchId, request: FetchRequest) {
        self.backend.inner.start(id, request);
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

    use super::super::fetch_core::FetchError;

    struct CompletionAfterIdle {
        polls: Cell<u8>,
    }

    impl NetworkBackend for CompletionAfterIdle {
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

    #[test]
    fn rechecks_for_completion_after_observed_idle() {
        let network = CompletionAware::new(CompletionAfterIdle {
            polls: Cell::new(0),
        });

        assert!(
            network.is_busy(),
            "a completion that appears after the first poll must remain visible"
        );
        let ready = network.poll();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, 7);
    }

    struct ImmediatelyReady {
        ready: RefCell<Vec<FetchCompletion>>,
    }

    impl NetworkBackend for ImmediatelyReady {
        fn start(&self, _id: FetchId, _request: FetchRequest) {}

        fn poll(&self) -> Vec<FetchCompletion> {
            std::mem::take(&mut *self.ready.borrow_mut())
        }
    }

    #[test]
    fn cancellation_removes_already_buffered_completion() {
        let network = CompletionAware::new(ImmediatelyReady {
            ready: RefCell::new(vec![FetchCompletion {
                id: 9,
                result: Err(FetchError::Aborted),
            }]),
        });

        assert!(network.is_busy());
        network.cancel(9);
        assert!(network.poll().is_empty());
    }
}
