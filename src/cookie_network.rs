// ============================================================
//  cookie_network.rs — HTTP cookie policy around NetworkBackend
// ============================================================

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use crate::cookie::CookieJar;
use crate::cookie_same_site::SameSiteRequestContext;
use crate::eventloop::Clock;
use crate::net::fetch::Method;
use crate::net::{FetchCompletion, FetchId, FetchRequest, FetchResponse, NetworkBackend};

/// Shared cookie storage for one browser session.
pub type CookieJarRef = Rc<RefCell<CookieJar>>;

/// Whether one request participates in the browser cookie session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookieCredentials {
    /// Send eligible stored cookies and accept response Set-Cookie state.
    Include,
    /// Send no cookies and ignore response Set-Cookie state.
    Omit,
}

/// Browser-owned cookie policy attached to one FetchId before the request starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CookieRequestPolicy {
    pub credentials: CookieCredentials,
    pub same_site: SameSiteRequestContext,
}

impl CookieRequestPolicy {
    pub const fn new(
        credentials: CookieCredentials,
        same_site: SameSiteRequestContext,
    ) -> CookieRequestPolicy {
        CookieRequestPolicy {
            credentials,
            same_site,
        }
    }

    /// Backward-compatible policy for existing Fetch callers: participate in
    /// the cookie session and treat the request as same-site.
    pub const fn same_site(method: Method) -> CookieRequestPolicy {
        CookieRequestPolicy::new(
            CookieCredentials::Include,
            SameSiteRequestContext::same_site(method),
        )
    }

    /// Credentials-omit policy with an explicit SameSite request context.
    pub const fn omit(context: SameSiteRequestContext) -> CookieRequestPolicy {
        CookieRequestPolicy::new(CookieCredentials::Omit, context)
    }
}

/// Adds RFC 6265bis cookie send/store behavior around any network backend.
///
/// The wrapped backend remains responsible only for transport. This decorator
/// owns browser policy at the request/response boundary:
///
/// - outgoing HTTP(S) requests receive context-eligible jar cookies;
/// - script-provided Cookie values are discarded rather than trusted;
/// - request-specific credentials=omit suppresses both outgoing Cookie and
///   incoming Set-Cookie state;
/// - every accepted Set-Cookie response line is processed independently with
///   the response URL retained as storage context;
/// - insecure responses cannot overlay existing Secure cookies;
/// - Set-Cookie is removed before the response reaches the script layer;
/// - request policy is cleared after completion or cancellation.
///
/// Policies are keyed by FetchId and are deliberately kept out of FetchRequest
/// headers, so page content cannot forge browser-internal cookie decisions.
pub struct CookieNetwork {
    inner: Rc<dyn NetworkBackend>,
    jar: CookieJarRef,
    clock: Rc<dyn Clock>,
    request_policies: RefCell<HashMap<FetchId, CookieRequestPolicy>>,
}

impl CookieNetwork {
    pub fn new(
        inner: Rc<dyn NetworkBackend>,
        jar: CookieJarRef,
        clock: Rc<dyn Clock>,
    ) -> CookieNetwork {
        CookieNetwork {
            inner,
            jar,
            clock,
            request_policies: RefCell::new(HashMap::new()),
        }
    }

    pub fn with_new_jar(inner: Rc<dyn NetworkBackend>, clock: Rc<dyn Clock>) -> CookieNetwork {
        CookieNetwork::new(inner, Rc::new(RefCell::new(CookieJar::new())), clock)
    }

    pub fn jar(&self) -> CookieJarRef {
        self.jar.clone()
    }

    pub fn inner(&self) -> &Rc<dyn NetworkBackend> {
        &self.inner
    }

    /// Attach browser-owned cookie policy to an id before `start` is called.
    /// Re-registering an id replaces its previous pending policy.
    pub fn set_request_policy(
        &self,
        id: FetchId,
        policy: CookieRequestPolicy,
    ) -> Option<CookieRequestPolicy> {
        self.request_policies.borrow_mut().insert(id, policy)
    }

    /// Remove a pending policy without cancelling transport.
    pub fn clear_request_policy(&self, id: FetchId) -> Option<CookieRequestPolicy> {
        self.request_policies.borrow_mut().remove(&id)
    }

    /// Number of explicitly registered policies waiting for completion/cancel.
    pub fn pending_policy_count(&self) -> usize {
        self.request_policies.borrow().len()
    }

    pub fn request_policy(&self, id: FetchId) -> Option<CookieRequestPolicy> {
        self.request_policies.borrow().get(&id).copied()
    }

    fn now_ms(&self) -> u64 {
        self.clock.now_ms().max(0.0) as u64
    }

    fn policy_for_start(&self, id: FetchId, method: Method) -> CookieRequestPolicy {
        self.request_policies
            .borrow()
            .get(&id)
            .copied()
            .unwrap_or_else(|| CookieRequestPolicy::same_site(method))
    }

    fn prepare_request(
        &self,
        id: FetchId,
        mut request: FetchRequest,
    ) -> FetchRequest {
        if !matches!(request.url.scheme(), "http" | "https") {
            return request;
        }

        // Cookie is browser-owned even under credentials=omit: a page-authored
        // value must never survive as a back door around the jar policy.
        request.headers.delete("cookie");
        let policy = self.policy_for_start(id, request.method);
        if policy.credentials == CookieCredentials::Omit {
            return request;
        }

        if let Some(value) = self.jar.borrow().get_http_cookie_header_for_context(
            &request.url,
            self.now_ms(),
            policy.same_site,
        ) {
            request.headers.insert_raw("cookie", &value);
        }
        request
    }

    fn absorb_response(
        &self,
        response: &mut FetchResponse,
        credentials: CookieCredentials,
    ) {
        if matches!(response.url.scheme(), "http" | "https")
            && credentials == CookieCredentials::Include
        {
            // Keep individual Set-Cookie lines separate. HeaderMap::get() joins
            // duplicates with commas, which is not valid for Set-Cookie.
            let values: Vec<String> = response
                .headers
                .iter()
                .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
                .map(|(_, value)| value.to_string())
                .collect();
            let now_ms = self.now_ms();
            if !values.is_empty() {
                let mut jar = self.jar.borrow_mut();
                for value in values {
                    jar.store_set_cookie(&value, &response.url, now_ms);
                }
            }
        }

        // Set-Cookie is forbidden to Fetch-visible script regardless of whether
        // credentials policy allowed storage.
        response.headers.delete("set-cookie");
    }
}

impl NetworkBackend for CookieNetwork {
    fn start(&self, id: FetchId, request: FetchRequest) {
        self.inner.start(id, self.prepare_request(id, request));
    }

    fn poll(&self) -> Vec<FetchCompletion> {
        let mut completions = self.inner.poll();
        for completion in &mut completions {
            let policy = self.request_policies.borrow_mut().remove(&completion.id);
            let credentials = policy
                .map(|policy| policy.credentials)
                .unwrap_or(CookieCredentials::Include);
            if let Ok(response) = &mut completion.result {
                self.absorb_response(response, credentials);
            }
        }
        completions
    }

    fn cancel(&self, id: FetchId) {
        self.request_policies.borrow_mut().remove(&id);
        self.inner.cancel(id);
    }

    fn is_busy(&self) -> bool {
        self.inner.is_busy()
    }

    fn wait(&self, timeout: Duration) -> bool {
        self.inner.wait(timeout)
    }
}
