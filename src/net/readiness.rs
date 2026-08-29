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
use std::collections::HashSet;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
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

    fn start(&self, id: FetchId, request: FetchRequest) {
        self.inner.start(id, request);
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

/// Worker backend that deliberately performs one `ResourceLoader::fetch_once`
/// exchange per started request.
///
/// This is kept separate from the legacy redirect-following core so adding the
/// single-hop capability cannot silently change existing `ThreadedNetwork::new`
/// callers. Redirect orchestration can opt in explicitly and observe 3xx
/// responses while keeping blocking I/O off the browser thread.
struct SingleHopCore {
    loader: Arc<dyn ResourceLoader>,
    sender: Sender<FetchCompletion>,
    receiver: Receiver<FetchCompletion>,
    ready: RefCell<Vec<FetchCompletion>>,
    cancelled: RefCell<HashSet<FetchId>>,
    workers: RefCell<Vec<JoinHandle<()>>>,
}

impl SingleHopCore {
    fn new(loader: Arc<dyn ResourceLoader>) -> Self {
        let (sender, receiver) = channel();
        Self {
            loader,
            sender,
            receiver,
            ready: RefCell::new(Vec::new()),
            cancelled: RefCell::new(HashSet::new()),
            workers: RefCell::new(Vec::new()),
        }
    }

    fn reap(&self) {
        self.workers
            .borrow_mut()
            .retain(|handle| !handle.is_finished());
    }
}

impl NetworkBackend for SingleHopCore {
    fn start(&self, id: FetchId, request: FetchRequest) {
        let loader = self.loader.clone();
        let sender = self.sender.clone();
        let worker = std::thread::spawn(move || {
            let result = loader.fetch_once(&request);
            let _ = sender.send(FetchCompletion { id, result });
        });
        self.workers.borrow_mut().push(worker);
    }

    fn poll(&self) -> Vec<FetchCompletion> {
        self.reap();
        let mut arrived = std::mem::take(&mut *self.ready.borrow_mut());
        arrived.extend(self.receiver.try_iter());
        let mut cancelled = self.cancelled.borrow_mut();
        arrived
            .into_iter()
            .filter(|completion| !cancelled.remove(&completion.id))
            .collect()
    }

    fn cancel(&self, id: FetchId) {
        self.cancelled.borrow_mut().insert(id);
    }

    fn is_busy(&self) -> bool {
        self.reap();
        !self.workers.borrow().is_empty() || !self.ready.borrow().is_empty()
    }

    fn wait(&self, timeout: Duration) -> bool {
        match self.receiver.recv_timeout(timeout) {
            Ok(completion) => {
                self.ready.borrow_mut().push(completion);
                true
            }
            Err(_) => false,
        }
    }
}

enum ThreadedBackend {
    Follow(CompletionAware<fetch_core::ThreadedNetwork>),
    SingleHop(CompletionAware<SingleHopCore>),
}

impl ThreadedBackend {
    fn start(&self, id: FetchId, request: FetchRequest) {
        match self {
            ThreadedBackend::Follow(backend) => backend.start(id, request),
            ThreadedBackend::SingleHop(backend) => backend.start(id, request),
        }
    }

    fn poll(&self) -> Vec<FetchCompletion> {
        match self {
            ThreadedBackend::Follow(backend) => backend.poll(),
            ThreadedBackend::SingleHop(backend) => backend.poll(),
        }
    }

    fn cancel(&self, id: FetchId) {
        match self {
            ThreadedBackend::Follow(backend) => backend.cancel(id),
            ThreadedBackend::SingleHop(backend) => backend.cancel(id),
        }
    }

    fn is_busy(&self) -> bool {
        match self {
            ThreadedBackend::Follow(backend) => backend.is_busy(),
            ThreadedBackend::SingleHop(backend) => backend.is_busy(),
        }
    }

    fn wait(&self, timeout: Duration) -> bool {
        match self {
            ThreadedBackend::Follow(backend) => backend.wait(timeout),
            ThreadedBackend::SingleHop(backend) => backend.wait(timeout),
        }
    }
}

/// Public threaded backend with race-free ready/busy observation.
pub struct ThreadedNetwork {
    backend: ThreadedBackend,
}

impl ThreadedNetwork {
    /// Create the legacy redirect-following threaded backend.
    pub fn new(loader: Arc<dyn ResourceLoader>) -> Self {
        Self {
            backend: ThreadedBackend::Follow(CompletionAware::new(
                fetch_core::ThreadedNetwork::new(loader),
            )),
        }
    }

    /// Create a threaded backend that performs exactly one loader exchange.
    ///
    /// HTTP 301/302/303/307/308 responses remain visible to the caller instead
    /// of being consumed inside the loader. This is the asynchronous primitive
    /// used by higher redirect layers that need to run Cookie/HSTS/referrer
    /// policy between hops.
    pub fn new_single_hop(loader: Arc<dyn ResourceLoader>) -> Self {
        Self {
            backend: ThreadedBackend::SingleHop(CompletionAware::new(SingleHopCore::new(loader))),
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
