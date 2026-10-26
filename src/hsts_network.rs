// ============================================================
//  hsts_network.rs — HSTS enforcement around NetworkBackend
// ============================================================

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use crate::eventloop::Clock;
use crate::hsts::HstsCache;
use crate::net::{FetchCompletion, FetchId, FetchRequest, NetworkBackend};

/// Shared HSTS state for one browser session/profile.
pub type HstsCacheRef = Rc<RefCell<HstsCache>>;

/// Applies learned HTTP Strict Transport Security state at the network boundary.
///
/// The wrapped backend remains transport-only. This decorator owns the two
/// user-agent policy transitions required by RFC 6797:
///
/// - before dispatch, an HTTP request to a Known HSTS Host is rewritten to
///   HTTPS (including the RFC port mapping performed by [`HstsCache`]);
/// - after a successful HTTPS response arrives, the first
///   `Strict-Transport-Security` field is processed into the shared cache.
///
/// Keeping HSTS here means every caller of the decorated backend gets the same
/// policy without teaching JavaScript, cookies, or an individual HTTP client
/// about HSTS.
pub struct HstsNetwork {
    inner: Rc<dyn NetworkBackend>,
    cache: HstsCacheRef,
    clock: Rc<dyn Clock>,
}

impl HstsNetwork {
    pub fn new(
        inner: Rc<dyn NetworkBackend>,
        cache: HstsCacheRef,
        clock: Rc<dyn Clock>,
    ) -> HstsNetwork {
        HstsNetwork { inner, cache, clock }
    }

    pub fn with_new_cache(inner: Rc<dyn NetworkBackend>, clock: Rc<dyn Clock>) -> HstsNetwork {
        HstsNetwork::new(inner, Rc::new(RefCell::new(HstsCache::new())), clock)
    }

    pub fn cache(&self) -> HstsCacheRef {
        self.cache.clone()
    }

    pub fn inner(&self) -> &Rc<dyn NetworkBackend> {
        &self.inner
    }

    fn now_ms(&self) -> u64 {
        self.clock.now_ms().max(0.0) as u64
    }

    fn prepare_request(&self, mut request: FetchRequest) -> FetchRequest {
        request.url = self.cache.borrow().upgrade_url(&request.url, self.now_ms());
        request
    }

    fn absorb_completion(&self, completion: &mut FetchCompletion) {
        let Ok(response) = &mut completion.result else {
            return;
        };

        // RFC 6797 §8.1 requires a UA receiving duplicate STS fields over
        // secure transport to process only the first one. HeaderMap::get()
        // would join duplicates with a comma, so inspect the ordered fields.
        let first_sts = response
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("strict-transport-security"))
            .map(|(_, value)| value.to_string());

        if let Some(value) = first_sts {
            self.cache
                .borrow_mut()
                .observe_response(&response.url, &value, self.now_ms());
        }
    }
}

impl NetworkBackend for HstsNetwork {
    fn start(&self, id: FetchId, request: FetchRequest) {
        self.inner.start(id, self.prepare_request(request));
    }

    fn poll(&self) -> Vec<FetchCompletion> {
        let mut completions = self.inner.poll();
        for completion in &mut completions {
            self.absorb_completion(completion);
        }
        completions
    }

    fn cancel(&self, id: FetchId) {
        self.inner.cancel(id);
    }

    fn is_busy(&self) -> bool {
        self.inner.is_busy()
    }

    fn wait(&self, timeout: Duration) -> bool {
        self.inner.wait(timeout)
    }
}
