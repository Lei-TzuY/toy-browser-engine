// ============================================================
//  session_network.rs — canonical browser-session network stack
// ============================================================

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use crate::cookie::CookieJar;
use crate::cookie_network::{CookieJarRef, CookieNetwork, CookiePolicyRegistry};
use crate::eventloop::Clock;
use crate::hsts::HstsCache;
use crate::hsts_network::{HstsCacheRef, HstsNetwork};
use crate::net::{FetchCompletion, FetchId, FetchRequest, NetworkBackend};

/// Canonical composition of browser-owned request policies around a transport.
///
/// HSTS intentionally sits *outside* CookieNetwork. That ordering matters:
/// an HTTP URL upgraded by HSTS must become HTTPS before cookie selection so
/// Secure cookies are eligible for the request that actually reaches transport.
pub struct SessionNetwork {
    hsts: HstsNetwork,
    cookie_jar: CookieJarRef,
    hsts_cache: HstsCacheRef,
    cookie_policies: CookiePolicyRegistry,
}

impl SessionNetwork {
    pub fn new(
        transport: Rc<dyn NetworkBackend>,
        cookie_jar: CookieJarRef,
        hsts_cache: HstsCacheRef,
        clock: Rc<dyn Clock>,
    ) -> SessionNetwork {
        let cookie_policies = CookiePolicyRegistry::new();
        let cookie: Rc<dyn NetworkBackend> = Rc::new(CookieNetwork::with_policy_registry(
            transport,
            cookie_jar.clone(),
            clock.clone(),
            cookie_policies.clone(),
        ));
        let hsts = HstsNetwork::new(cookie, hsts_cache.clone(), clock);
        SessionNetwork {
            hsts,
            cookie_jar,
            hsts_cache,
            cookie_policies,
        }
    }

    /// Build an isolated browser-session stack with fresh cookie and HSTS state.
    pub fn with_new_state(
        transport: Rc<dyn NetworkBackend>,
        clock: Rc<dyn Clock>,
    ) -> SessionNetwork {
        let cookie_jar = Rc::new(RefCell::new(CookieJar::with_clock(clock.clone())));
        let hsts_cache = Rc::new(RefCell::new(HstsCache::new()));
        SessionNetwork::new(transport, cookie_jar, hsts_cache, clock)
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
        self.hsts.start(id, request);
    }

    fn poll(&self) -> Vec<FetchCompletion> {
        self.hsts.poll()
    }

    fn cancel(&self, id: FetchId) {
        self.hsts.cancel(id);
    }

    fn is_busy(&self) -> bool {
        self.hsts.is_busy()
    }

    fn wait(&self, timeout: Duration) -> bool {
        self.hsts.wait(timeout)
    }
}
