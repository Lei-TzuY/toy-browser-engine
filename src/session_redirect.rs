// ============================================================
//  session_redirect.rs — per-hop async Fetch redirect orchestration
// ============================================================
//
// This layer sits *outside* the per-hop HSTS/Cookie decorators. Every accepted
// Location therefore becomes a fresh request through those policies instead of
// being followed inside the raw HTTP transport.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use crate::cookie_network::{
    CookieCredentials, CookiePolicyRegistry, CookieRequestPolicy,
};
use crate::cookie_same_site::SameSiteRequestContext;
use crate::eventloop::Clock;
use crate::hsts_network::HstsCacheRef;
use crate::net::{FetchCompletion, FetchError, FetchId, FetchRequest, NetworkBackend, Origin};
use crate::redirect_policy::{RedirectError, RedirectPlanner};

struct RedirectChain {
    /// Browser-policy-free request shape for the current hop. Its URL is kept
    /// HSTS-effective so Location resolution and same-origin checks use the URL
    /// that actually reached transport.
    current_request: FetchRequest,
    initial_origin: Origin,
    credentials: CookieCredentials,
    planner: RedirectPlanner,
}

/// Redirect orchestration for same-origin asynchronous Fetch.
///
/// `inner` must represent exactly one transport hop wrapped in the session's
/// HSTS and Cookie policy. Intermediate responses are therefore absorbed by
/// those decorators before this state machine sees them. When a redirect is
/// accepted, this layer re-arms the original credentials mode for the same
/// FetchId and sends the planner's next request back through the same per-hop
/// policy stack.
pub(crate) struct SessionRedirectNetwork {
    inner: Rc<dyn NetworkBackend>,
    cookie_policies: CookiePolicyRegistry,
    hsts_cache: HstsCacheRef,
    clock: Rc<dyn Clock>,
    chains: RefCell<HashMap<FetchId, RedirectChain>>,
}

impl SessionRedirectNetwork {
    pub(crate) fn new(
        inner: Rc<dyn NetworkBackend>,
        cookie_policies: CookiePolicyRegistry,
        hsts_cache: HstsCacheRef,
        clock: Rc<dyn Clock>,
    ) -> SessionRedirectNetwork {
        SessionRedirectNetwork {
            inner,
            cookie_policies,
            hsts_cache,
            clock,
            chains: RefCell::new(HashMap::new()),
        }
    }

    fn now_ms(&self) -> u64 {
        self.clock.now_ms().max(0.0) as u64
    }

    fn effective_url(&self, request: &FetchRequest) -> crate::net::Url {
        self.hsts_cache
            .borrow()
            .upgrade_url(&request.url, self.now_ms())
    }

    fn request_policy(&self, id: FetchId, request: &FetchRequest) -> CookieRequestPolicy {
        self.cookie_policies
            .get(id)
            .unwrap_or_else(|| CookieRequestPolicy::same_site(request.method))
    }

    fn redirect_policy(credentials: CookieCredentials, request: &FetchRequest) -> CookieRequestPolicy {
        CookieRequestPolicy::new(
            credentials,
            SameSiteRequestContext::same_site(request.method),
        )
    }

    fn redirect_error(error: RedirectError, response_url: &crate::net::Url) -> FetchError {
        match error {
            RedirectError::InvalidLocation(_) => {
                FetchError::MalformedResponse(response_url.to_string())
            }
            RedirectError::UnsupportedScheme(scheme) => FetchError::UnsupportedScheme(scheme),
            RedirectError::TooManyRedirects(url) => FetchError::TooManyRedirects(url),
        }
    }
}

impl NetworkBackend for SessionRedirectNetwork {
    fn start(&self, id: FetchId, request: FetchRequest) {
        let policy = self.request_policy(id, &request);
        let mut effective_request = request.clone();
        effective_request.url = self.effective_url(&request);
        let initial_origin = Origin::of(&effective_request.url);

        self.chains.borrow_mut().insert(
            id,
            RedirectChain {
                current_request: effective_request,
                initial_origin,
                credentials: policy.credentials,
                planner: RedirectPlanner::default(),
            },
        );
        self.inner.start(id, request);
    }

    fn poll(&self) -> Vec<FetchCompletion> {
        let completions = self.inner.poll();
        let mut visible = Vec::new();

        for mut completion in completions {
            let id = completion.id;
            let Some(mut chain) = self.chains.borrow_mut().remove(&id) else {
                visible.push(completion);
                continue;
            };

            let response_url = match completion.result.as_ref() {
                Ok(response) => response.url.clone(),
                Err(_) => {
                    visible.push(completion);
                    continue;
                }
            };

            // HSTS may have rewritten the request before transport. The single
            // hop response records that effective URL, which is the correct base
            // for Location resolution and the current-hop origin.
            chain.current_request.url = response_url.clone();

            let next = {
                let response = completion.result.as_ref().expect("checked above");
                chain.planner.next_request(&chain.current_request, response)
            };

            match next {
                Ok(None) => {
                    if let Ok(response) = &mut completion.result {
                        response.redirected = chain.planner.followed() > 0;
                    }
                    visible.push(completion);
                }
                Err(error) => {
                    completion.result = Err(Self::redirect_error(error, &response_url));
                    visible.push(completion);
                }
                Ok(Some(next_request)) => {
                    // Fetch is same-origin-only today. Classify the redirect
                    // target *after* learned HSTS is applied, matching Browser's
                    // top-level HSTS-before-SameSite/origin ordering.
                    let effective_next = self.effective_url(&next_request);
                    let next_origin = Origin::of(&effective_next);
                    if next_origin != chain.initial_origin {
                        completion.result = Err(FetchError::Blocked(format!(
                            "redirect from {} to {} crossed origin",
                            chain.initial_origin.header_value(),
                            effective_next
                        )));
                        visible.push(completion);
                        continue;
                    }

                    // CookieNetwork consumes per-FetchId policy when one hop
                    // completes. Re-arm the same credentials mode before the
                    // next hop, with a same-origin subresource context updated
                    // for any redirect method rewrite.
                    self.cookie_policies.set(
                        id,
                        Self::redirect_policy(chain.credentials, &next_request),
                    );

                    chain.current_request = next_request.clone();
                    chain.current_request.url = effective_next;
                    self.chains.borrow_mut().insert(id, chain);
                    self.inner.start(id, next_request);
                }
            }
        }

        visible
    }

    fn cancel(&self, id: FetchId) {
        self.chains.borrow_mut().remove(&id);
        self.cookie_policies.remove(id);
        self.inner.cancel(id);
    }

    fn is_busy(&self) -> bool {
        !self.chains.borrow().is_empty() || self.inner.is_busy()
    }

    fn wait(&self, timeout: Duration) -> bool {
        self.inner.wait(timeout)
    }
}
