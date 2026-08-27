// ============================================================
//  fetch_final.rs — public fetch facade
// ============================================================
//
// Keep the request/response vocabulary and deterministic test backends from
// the raw implementation, but make the public threaded/routing backends use
// the completion-tracked wrappers from `net_final` too. This prevents callers
// of `net::fetch::ThreadedNetwork` from bypassing the lifecycle guarantee that
// `net::ThreadedNetwork` provides.

pub use crate::net_base::fetch::{
    reason_phrase, FetchCompletion, FetchError, FetchId, FetchRequest, FetchResponse, HeaderError,
    HeaderMap, LocalNetwork, ManualNetwork, Method, NetworkBackend, OfflineNetwork, Origin,
    MAX_IN_FLIGHT_FETCHES,
};

pub use super::{DefaultNetwork, ThreadedNetwork};

/// What a page has in flight.
///
/// This mirrors the raw registry API but strengthens one allocator invariant:
/// wrapping the `u64` id counter must never hand out an id that is still live.
/// A long-lived page can therefore cross the counter boundary without making
/// two pending requests indistinguishable to completion/cancellation logic.
#[derive(Debug)]
pub struct FetchRegistry<T> {
    next_id: FetchId,
    pending: Vec<(FetchId, T)>,
    /// Requests waiting to be handed to a backend.
    outbox: Vec<(FetchId, FetchRequest)>,
    /// Requests whose answer is no longer wanted, waiting to be cancelled.
    cancelled: Vec<FetchId>,
    limit: usize,
}

impl<T> Default for FetchRegistry<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> FetchRegistry<T> {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            pending: Vec::new(),
            outbox: Vec::new(),
            cancelled: Vec::new(),
            limit: MAX_IN_FLIGHT_FETCHES,
        }
    }

    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit.max(1);
        self
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Pick the next non-zero id that cannot currently be confused with live
    /// work. `pending` covers both queued and already-sent requests; ids still
    /// waiting in `cancelled` are also reserved until the backend has at least
    /// been told to suppress their answers.
    fn allocate_id(&mut self) -> FetchId {
        let mut candidate = self.next_id.max(1);
        loop {
            let live = self
                .pending
                .iter()
                .any(|(pending, _)| *pending == candidate);
            let cancellation_pending = self.cancelled.contains(&candidate);
            if !live && !cancellation_pending {
                self.next_id = candidate.wrapping_add(1).max(1);
                return candidate;
            }
            candidate = candidate.wrapping_add(1).max(1);
        }
    }

    /// Record a request and queue it for the network.
    ///
    /// Fails when the page is already at the configured in-flight limit, so a
    /// runaway loop is refused immediately instead of building a backlog.
    pub fn start(&mut self, request: FetchRequest, handle: T) -> Result<FetchId, FetchError> {
        if self.pending.len() >= self.limit {
            return Err(FetchError::TooManyRequests);
        }
        let id = self.allocate_id();
        self.pending.push((id, handle));
        self.outbox.push((id, request));
        Ok(id)
    }

    /// Take the requests waiting to be sent.
    pub fn take_outbox(&mut self) -> Vec<(FetchId, FetchRequest)> {
        std::mem::take(&mut self.outbox)
    }

    /// Claim the handle for a finished request, if it is still wanted.
    pub fn take(&mut self, id: FetchId) -> Option<T> {
        let index = self
            .pending
            .iter()
            .position(|(pending, _)| *pending == id)?;
        Some(self.pending.remove(index).1)
    }

    /// Claim every handle matching a predicate — how an abort finds its
    /// request without the registry knowing what a signal is.
    pub fn take_where(&mut self, mut wanted: impl FnMut(&T) -> bool) -> Vec<(FetchId, T)> {
        let mut taken = Vec::new();
        let mut index = 0;
        while index < self.pending.len() {
            if wanted(&self.pending[index].1) {
                taken.push(self.pending.remove(index));
            } else {
                index += 1;
            }
        }

        let ids: Vec<FetchId> = taken.iter().map(|(id, _)| *id).collect();

        // Requests still present in the outbox have never reached a backend.
        // Abort can simply erase them: asking the backend to cancel an id it
        // has never seen creates fake lifecycle state and unnecessarily keeps
        // that id reserved across allocator wrap. Only ids absent from the
        // outbox are considered already dispatched and need a real cancel.
        let unsent: Vec<FetchId> = self
            .outbox
            .iter()
            .filter_map(|(id, _)| ids.contains(id).then_some(*id))
            .collect();
        self.outbox.retain(|(id, _)| !ids.contains(id));
        self.cancelled
            .extend(ids.iter().copied().filter(|id| !unsent.contains(id)));
        taken
    }

    /// Take the ids whose answers should no longer be delivered.
    pub fn take_cancellations(&mut self) -> Vec<FetchId> {
        std::mem::take(&mut self.cancelled)
    }

    pub fn contains(&self, id: FetchId) -> bool {
        self.pending.iter().any(|(pending, _)| *pending == id)
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// True while anything is in flight or waiting to be sent.
    pub fn has_pending_work(&self) -> bool {
        !self.pending.is_empty() || !self.outbox.is_empty()
    }

    /// Drop everything — what navigating away from a page does. The handles go
    /// with it, so no promise from the old page can ever be settled.
    ///
    /// Requests already removed from `pending` by an abort may still be waiting
    /// in `cancelled` for the next network-dispatch turn. They must be returned
    /// here too: navigation can happen before that turn, and silently clearing
    /// those ids would leave an already-sent backend request alive after its
    /// document disappeared.
    pub fn clear(&mut self) -> Vec<FetchId> {
        let mut ids: Vec<FetchId> = self.pending.iter().map(|(id, _)| *id).collect();
        ids.append(&mut self.cancelled);
        ids.sort_unstable();
        ids.dedup();

        self.pending.clear();
        self.outbox.clear();
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(path: &str) -> FetchRequest {
        FetchRequest::get(super::super::Url::parse(&format!("demo:///{path}")).unwrap())
    }

    #[test]
    fn id_wrap_skips_a_still_pending_request() {
        let mut registry = FetchRegistry::new().with_limit(4);
        let first = registry.start(request("first"), "first").unwrap();
        assert_eq!(first, 1);

        registry.next_id = FetchId::MAX;
        let last = registry.start(request("last"), "last").unwrap();
        assert_eq!(last, FetchId::MAX);

        let wrapped = registry.start(request("wrapped"), "wrapped").unwrap();
        assert_eq!(wrapped, 2);
        assert_eq!(registry.len(), 3);
        assert!(registry.contains(first));
        assert!(registry.contains(last));
        assert!(registry.contains(wrapped));
    }

    #[test]
    fn wrap_does_not_reuse_an_id_waiting_for_backend_cancellation() {
        let mut registry = FetchRegistry::new().with_limit(4);
        let doomed = registry.start(request("doomed"), "abort").unwrap();
        assert_eq!(doomed, 1);
        registry.take_outbox(); // this request has been dispatched
        let taken = registry.take_where(|handle| *handle == "abort");
        assert_eq!(taken, vec![(1, "abort")]);

        registry.next_id = FetchId::MAX;
        assert_eq!(registry.start(request("last"), "last").unwrap(), FetchId::MAX);
        assert_eq!(
            registry.start(request("wrapped"), "wrapped").unwrap(),
            2,
            "id 1 stays reserved until its cancellation is drained"
        );

        assert_eq!(registry.take_cancellations(), vec![1]);
    }

    #[test]
    fn aborting_an_unsent_request_never_queues_backend_cancellation() {
        let mut registry = FetchRegistry::new().with_limit(4);
        let id = registry.start(request("unsent"), "abort").unwrap();
        assert_eq!(registry.take_where(|handle| *handle == "abort"), vec![(id, "abort")]);

        assert!(registry.take_outbox().is_empty());
        assert!(registry.take_cancellations().is_empty());
        assert!(registry.is_empty());
    }

    #[test]
    fn aborting_a_dispatched_request_queues_backend_cancellation() {
        let mut registry = FetchRegistry::new().with_limit(4);
        let id = registry.start(request("sent"), "abort").unwrap();
        let outbox = registry.take_outbox();
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox[0].0, id);

        assert_eq!(registry.take_where(|handle| *handle == "abort"), vec![(id, "abort")]);
        assert_eq!(registry.take_cancellations(), vec![id]);
        assert!(registry.is_empty());
    }

    #[test]
    fn clear_preserves_an_unflushed_cancellation_for_the_backend() {
        let mut registry = FetchRegistry::new().with_limit(4);
        let sent = registry.start(request("sent"), "abort").unwrap();
        assert_eq!(registry.take_outbox().len(), 1, "request has left the registry outbox");

        let aborted = registry.take_where(|handle| *handle == "abort");
        assert_eq!(aborted, vec![(sent, "abort")]);
        assert!(registry.is_empty());

        assert_eq!(registry.clear(), vec![sent]);
        assert!(registry.take_cancellations().is_empty());
    }

    #[test]
    fn clear_returns_pending_and_queued_cancellation_ids_once_each() {
        let mut registry = FetchRegistry::new().with_limit(4);
        let keep = registry.start(request("keep"), "keep").unwrap();
        let doomed = registry.start(request("doomed"), "abort").unwrap();
        registry.take_outbox();
        registry.take_where(|handle| *handle == "abort");

        let mut ids = registry.clear();
        ids.sort_unstable();
        let mut expected = vec![keep, doomed];
        expected.sort_unstable();
        assert_eq!(ids, expected);
        assert!(registry.is_empty());
        assert!(!registry.has_pending_work());
        assert!(registry.take_cancellations().is_empty());
    }

    #[test]
    fn registry_keeps_existing_limit_outbox_and_take_semantics() {
        let mut registry = FetchRegistry::new().with_limit(2);
        let a = registry.start(request("a"), "a").unwrap();
        let b = registry.start(request("b"), "b").unwrap();
        assert_eq!(
            registry.start(request("c"), "c"),
            Err(FetchError::TooManyRequests)
        );

        let outbox = registry.take_outbox();
        assert_eq!(outbox.iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![a, b]);
        assert!(registry.take_outbox().is_empty());
        assert_eq!(registry.take(a), Some("a"));
        assert!(registry.contains(b));
        assert_eq!(registry.len(), 1);
    }
}
