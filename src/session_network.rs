// ============================================================
//  session_network.rs — canonical browser-session network stack
// ============================================================

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use crate::cookie::CookieJar;
use crate::cookie_network::{CookieJarRef, CookieNetwork, CookiePolicyRegistry};
use crate::eventloop::Clock;
use crate::hsts::HstsCache;
use crate::hsts_network::{HstsCacheRef, HstsNetwork};
use crate::net::{FetchCompletion, FetchError, FetchId, FetchRequest, NetworkBackend, Origin};
use crate::session_redirect::SessionRedirectNetwork;

/// Async Fetch currently implements same-origin mode only. The legacy session
/// constructor keeps its existing final-response redirect guard so embedders
/// that still provide redirect-following transports retain the same behavior.
struct SameOriginRedirectNetwork {
    inner: Rc<dyn NetworkBackend>,
    request_origins: RefCell<HashMap<FetchId, Origin>>,
}

impl SameOriginRedirectNetwork {
    fn new(inner: Rc<dyn NetworkBackend>) -> SameOriginRedirectNetwork {
        SameOriginRedirectNetwork {
            inner,
            request_origins: RefCell::new(HashMap::new()),
        }
    }
}

impl NetworkBackend for SameOriginRedirectNetwork {
    fn start(&self, id: FetchId, request: FetchRequest) {
        self.request_origins
            .borrow_mut()
            .insert(id, Origin::of(&request.url));
        self.inner.start(id, request);
    }

    fn poll(&self) -> Vec<FetchCompletion> {
        let mut completions = self.inner.poll();
        for completion in &mut completions {
            let Some(origin) = self.request_origins.borrow_mut().remove(&completion.id) else {
                continue;
            };
            let Ok(response) = &completion.result else {
                continue;
            };
            if response.redirected && origin != Origin::of(&response.url) {
                completion.result = Err(FetchError::Blocked(format!(
                    "redirect from {} to {} crossed origin",
                    origin.header_value(),
                    response.url
                )));
            }
        }
        completions
    }

    fn cancel(&self, id: FetchId) {
        self.request_origins.borrow_mut().remove(&id);
        self.inner.cancel(id);
    }

    fn is_busy(&self) -> bool {
        self.inner.is_busy()
    }

    fn wait(&self, timeout: Duration) -> bool {
        self.inner.wait(timeout)
    }
}

/// Enforces the transport contract of redirect-orchestrating sessions.
///
/// A redirecting SessionNetwork must see every 3xx response itself so Cookie,
/// HSTS, origin, and redirect policy can run between hops. A transport that
/// already followed redirects would skip those browser-policy boundaries. The
/// guard sits *inside* CookieNetwork/HstsNetwork so a pre-followed response is
/// rejected before its final Set-Cookie or STS headers can mutate session state.
struct SingleHopTransportGuard {
    inner: Rc<dyn NetworkBackend>,
}

impl SingleHopTransportGuard {
    fn new(inner: Rc<dyn NetworkBackend>) -> Self {
        Self { inner }
    }
}

impl NetworkBackend for SingleHopTransportGuard {
    fn start(&self, id: FetchId, request: FetchRequest) {
        self.inner.start(id, request);
    }

    fn poll(&self) -> Vec<FetchCompletion> {
        let mut completions = self.inner.poll();
        for completion in &mut completions {
            let Ok(response) = &completion.result else {
                continue;
            };
            if response.redirected {
                completion.result = Err(FetchError::MalformedResponse(format!(
                    "redirecting session requires a single-hop transport; {} was already redirected",
                    response.url
                )));
            }
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

/// Canonical composition of browser-owned request policies around a transport.
///
/// The existing constructors preserve the historical redirect-following
/// transport contract. The explicit redirecting constructors instead expect a
/// one-hop transport (for example `ThreadedNetwork::new_single_hop`) and move
/// redirect orchestration above HSTS/Cookie policy, so every accepted Location
/// becomes a fresh policy-processed request.
pub struct SessionNetwork {
    network: Rc<dyn NetworkBackend>,
    cookie_jar: CookieJarRef,
    hsts_cache: HstsCacheRef,
    cookie_policies: CookiePolicyRegistry,
}

impl SessionNetwork {
    /// Build the backward-compatible session stack around a transport that may
    /// already follow redirects internally.
    pub fn new(
        transport: Rc<dyn NetworkBackend>,
        cookie_jar: CookieJarRef,
        hsts_cache: HstsCacheRef,
        clock: Rc<dyn Clock>,
    ) -> SessionNetwork {
        let cookie_policies = CookiePolicyRegistry::new();
        let redirect_guard: Rc<dyn NetworkBackend> =
            Rc::new(SameOriginRedirectNetwork::new(transport));
        let cookie: Rc<dyn NetworkBackend> = Rc::new(CookieNetwork::with_policy_registry(
            redirect_guard,
            cookie_jar.clone(),
            clock.clone(),
            cookie_policies.clone(),
        ));
        let network: Rc<dyn NetworkBackend> =
            Rc::new(HstsNetwork::new(cookie, hsts_cache.clone(), clock));
        SessionNetwork {
            network,
            cookie_jar,
            hsts_cache,
            cookie_policies,
        }
    }

    /// Build a session stack whose transport exposes exactly one response per
    /// request. Redirects are followed here, above HSTS and Cookie policy.
    ///
    /// Per-hop ordering is:
    ///
    /// `redirect orchestration → HSTS upgrade → Cookie selection → transport`
    ///
    /// On completion the order reverses, so Set-Cookie and STS from an
    /// intermediate response are absorbed before the next Location is planned.
    pub fn new_redirecting(
        transport: Rc<dyn NetworkBackend>,
        cookie_jar: CookieJarRef,
        hsts_cache: HstsCacheRef,
        clock: Rc<dyn Clock>,
    ) -> SessionNetwork {
        let cookie_policies = CookiePolicyRegistry::new();
        let guarded_transport: Rc<dyn NetworkBackend> =
            Rc::new(SingleHopTransportGuard::new(transport));
        let cookie: Rc<dyn NetworkBackend> = Rc::new(CookieNetwork::with_policy_registry(
            guarded_transport,
            cookie_jar.clone(),
            clock.clone(),
            cookie_policies.clone(),
        ));
        let per_hop: Rc<dyn NetworkBackend> = Rc::new(HstsNetwork::new(
            cookie,
            hsts_cache.clone(),
            clock.clone(),
        ));
        let network: Rc<dyn NetworkBackend> = Rc::new(SessionRedirectNetwork::new(
            per_hop,
            cookie_policies.clone(),
            hsts_cache.clone(),
            clock,
        ));
        SessionNetwork {
            network,
            cookie_jar,
            hsts_cache,
            cookie_policies,
        }
    }

    /// Build an isolated backward-compatible browser-session stack with fresh
    /// cookie and HSTS state.
    pub fn with_new_state(
        transport: Rc<dyn NetworkBackend>,
        clock: Rc<dyn Clock>,
    ) -> SessionNetwork {
        let cookie_jar = Rc::new(RefCell::new(CookieJar::with_clock(clock.clone())));
        let hsts_cache = Rc::new(RefCell::new(HstsCache::new()));
        SessionNetwork::new(transport, cookie_jar, hsts_cache, clock)
    }

    /// Build an isolated redirect-orchestrating session around a one-hop
    /// transport, with fresh cookie and HSTS state.
    pub fn with_new_state_redirecting(
        transport: Rc<dyn NetworkBackend>,
        clock: Rc<dyn Clock>,
    ) -> SessionNetwork {
        let cookie_jar = Rc::new(RefCell::new(CookieJar::with_clock(clock.clone())));
        let hsts_cache = Rc::new(RefCell::new(HstsCache::new()));
        SessionNetwork::new_redirecting(transport, cookie_jar, hsts_cache, clock)
    }

    pub fn cookie_jar(&self) -> CookieJarRef {
        self.cookie_jar.clone()
    }

    pub fn hsts_cache(&self) -> HstsCacheRef {
        self.hsts_cache.clone()
    }

    pub fn cookie_policy_registry(&self) -> CookiePolicyRegistry {
        self.cookie_policies.clone()
    }
}

impl NetworkBackend for SessionNetwork {
    fn start(&self, id: FetchId, request: FetchRequest) {
        self.network.start(id, request);
    }

    fn poll(&self) -> Vec<FetchCompletion> {
        self.network.poll()
    }

    fn cancel(&self, id: FetchId) {
        self.network.cancel(id);
    }

    fn is_busy(&self) -> bool {
        self.network.is_busy()
    }

    fn wait(&self, timeout: Duration) -> bool {
        self.network.wait(timeout)
    }
}
