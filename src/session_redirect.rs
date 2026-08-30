// ============================================================
//  session_redirect.rs — per-hop async Fetch redirect orchestration
// ============================================================

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use crate::cookie_network::{
    CookieCredentials, CookieJarRef, CookiePolicyRegistry, CookieRequestPolicy,
};
use crate::cookie_same_site::SameSiteRequestContext;
use crate::eventloop::Clock;
use crate::fetch_cors::{
    cors_unsafe_request_header_names, is_cors_safelisted_method, validate_cors_response_origin,
    CORS_REDIRECT_ORIGIN_HEADER,
};
use crate::fetch_cors_preflight::{
    build_preflight_request, cache_allows as preflight_cache_allows, preflight_cookie_policy,
    store_permissions as store_preflight_permissions, validate_preflight_response,
};
use crate::fetch_cors_redirect::{
    FetchCorsRedirectPolicy, FetchCorsRedirectPolicyRegistry, FetchCredentialsMode,
    FetchRequestMode,
};
use crate::fetch_redirect_policy::{FetchRedirectMode, FetchRedirectPolicyRegistry};
use crate::hsts_network::HstsCacheRef;
use crate::net::{FetchCompletion, FetchError, FetchId, FetchRequest, NetworkBackend, Origin, Url};
use crate::redirect_policy::{RedirectError, RedirectPlanner};
use crate::referrer_policy::RedirectReferrerState;

struct RedirectPreflight {
    actual_request: FetchRequest,
    effective_url: Url,
    actual_cookie_policy: CookieRequestPolicy,
    serialized_origin: String,
    credentialed: bool,
    requested_method: crate::net::Method,
    requested_headers: Vec<String>,
}

struct RedirectChain {
    current_request: FetchRequest,
    initial_origin: Origin,
    fallback_credentials: CookieCredentials,
    redirect_mode: FetchRedirectMode,
    request_policy: Option<FetchCorsRedirectPolicy>,
    cors_tainted: bool,
    cors_origin: String,
    referrer: RedirectReferrerState,
    pending_preflight: Option<RedirectPreflight>,
    planner: RedirectPlanner,
}

pub(crate) struct SessionRedirectNetwork {
    inner: Rc<dyn NetworkBackend>,
    cookie_jar: CookieJarRef,
    cookie_policies: CookiePolicyRegistry,
    hsts_cache: HstsCacheRef,
    clock: Rc<dyn Clock>,
    redirect_policies: FetchRedirectPolicyRegistry,
    cors_redirect_policies: FetchCorsRedirectPolicyRegistry,
    chains: RefCell<HashMap<FetchId, RedirectChain>>,
}

impl SessionRedirectNetwork {
    pub(crate) fn new(
        inner: Rc<dyn NetworkBackend>,
        cookie_jar: CookieJarRef,
        cookie_policies: CookiePolicyRegistry,
        hsts_cache: HstsCacheRef,
        clock: Rc<dyn Clock>,
        redirect_policies: FetchRedirectPolicyRegistry,
        cors_redirect_policies: FetchCorsRedirectPolicyRegistry,
    ) -> SessionRedirectNetwork {
        SessionRedirectNetwork {
            inner,
            cookie_jar,
            cookie_policies,
            hsts_cache,
            clock,
            redirect_policies,
            cors_redirect_policies,
            chains: RefCell::new(HashMap::new()),
        }
    }

    fn now_ms(&self) -> u64 {
        self.clock.now_ms().max(0.0) as u64
    }

    fn effective_url(&self, request: &FetchRequest) -> Url {
        self.hsts_cache
            .borrow()
            .upgrade_url(&request.url, self.now_ms())
    }

    fn request_policy(&self, id: FetchId, request: &FetchRequest) -> CookieRequestPolicy {
        self.cookie_policies
            .get(id)
            .unwrap_or_else(|| CookieRequestPolicy::same_site(request.method))
    }

    fn conservative_same_site(source: &Url, target: &Url) -> bool {
        source.scheme() == target.scheme() && source.host().eq_ignore_ascii_case(target.host())
    }

    fn credentials_for_target(
        policy: &FetchCorsRedirectPolicy,
        target: &Url,
    ) -> CookieCredentials {
        match policy.credentials {
            FetchCredentialsMode::Omit => CookieCredentials::Omit,
            FetchCredentialsMode::Include => CookieCredentials::Include,
            FetchCredentialsMode::SameOrigin
                if Origin::of(&policy.source_url).can_fetch(target) =>
            {
                CookieCredentials::Include
            }
            FetchCredentialsMode::SameOrigin => CookieCredentials::Omit,
        }
    }

    fn redirect_cookie_policy(
        chain: &RedirectChain,
        target: &Url,
        method: crate::net::Method,
    ) -> CookieRequestPolicy {
        if let Some(policy) = &chain.request_policy {
            let credentials = Self::credentials_for_target(policy, target);
            let same_site = if Self::conservative_same_site(&policy.source_url, target) {
                SameSiteRequestContext::same_site(method)
            } else {
                SameSiteRequestContext::cross_site_subresource(method)
            };
            CookieRequestPolicy::new(credentials, same_site)
        } else {
            CookieRequestPolicy::new(
                chain.fallback_credentials,
                SameSiteRequestContext::same_site(method),
            )
        }
    }

    fn redirect_error(error: RedirectError, response_url: &Url) -> FetchError {
        match error {
            RedirectError::InvalidLocation(_) => {
                FetchError::MalformedResponse(response_url.to_string())
            }
            RedirectError::UnsupportedScheme(scheme) => FetchError::UnsupportedScheme(scheme),
            RedirectError::TooManyRedirects(url) => FetchError::TooManyRedirects(url),
        }
    }

    fn current_cors_response_must_pass(chain: &RedirectChain) -> bool {
        let Some(policy) = &chain.request_policy else {
            return false;
        };
        policy.mode == FetchRequestMode::Cors
            && !Origin::of(&policy.source_url).can_fetch(&chain.current_request.url)
    }

    fn is_preflight(request: &FetchRequest) -> bool {
        request.method == crate::net::Method::Options
            && request.headers.has("access-control-request-method")
    }

    /// Referrer Policy is evaluated against the HSTS-effective target, while
    /// the inner HSTS layer still owns the actual URL rewrite. Temporarily
    /// substitute that target only for Referer computation, then restore the
    /// authored URL before dispatch.
    fn prepare_referrer_for_effective_target(
        state: &RedirectReferrerState,
        request: &mut FetchRequest,
        effective_target: &Url,
    ) {
        let authored_url = request.url.clone();
        request.url = effective_target.clone();
        state.prepare_request(request);
        request.url = authored_url;
    }
}

impl NetworkBackend for SessionRedirectNetwork {
    fn start(&self, id: FetchId, mut request: FetchRequest) {
        let cookie_policy = self.request_policy(id, &request);
        let redirect_mode = self.redirect_policies.remove(id).unwrap_or_default();
        let request_policy = self.cors_redirect_policies.remove(id);
        let referrer = request_policy
            .as_ref()
            .map(|policy| policy.referrer.clone())
            .unwrap_or_else(RedirectReferrerState::no_referrer);
        let effective_url = self.effective_url(&request);
        Self::prepare_referrer_for_effective_target(&referrer, &mut request, &effective_url);
        let mut effective_request = request.clone();
        effective_request.url = effective_url;
        let initial_origin = Origin::of(&effective_request.url);
        let (cors_tainted, cors_origin) = match &request_policy {
            Some(policy) if policy.mode == FetchRequestMode::Cors => {
                let source_origin = Origin::of(&policy.source_url);
                (
                    !source_origin.can_fetch(&effective_request.url),
                    source_origin.header_value(),
                )
            }
            _ => (false, String::new()),
        };

        self.chains.borrow_mut().insert(
            id,
            RedirectChain {
                current_request: effective_request,
                initial_origin,
                fallback_credentials: cookie_policy.credentials,
                redirect_mode,
                request_policy,
                cors_tainted,
                cors_origin,
                referrer,
                pending_preflight: None,
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
            chain.current_request.url = response_url.clone();

            if let Some(preflight) = chain.pending_preflight.take() {
                let response = completion.result.as_ref().expect("checked above");
                if let Err(error) = validate_preflight_response(
                    &preflight.serialized_origin,
                    preflight.credentialed,
                    preflight.requested_method,
                    &preflight.requested_headers,
                    response,
                ) {
                    completion.result = Err(error);
                    visible.push(completion);
                    continue;
                }

                store_preflight_permissions(
                    &self.cookie_jar,
                    self.now_ms(),
                    &preflight.serialized_origin,
                    &preflight.actual_request.url,
                    preflight.credentialed,
                    response,
                );
                self.cookie_policies.set(id, preflight.actual_cookie_policy);
                chain.current_request = preflight.actual_request.clone();
                chain.current_request.url = preflight.effective_url;
                self.chains.borrow_mut().insert(id, chain);
                self.inner.start(id, preflight.actual_request);
                continue;
            }

            let next = {
                let response = completion.result.as_ref().expect("checked above");
                chain.planner.next_request(&chain.current_request, response)
            };

            match next {
                Ok(None) => {
                    if let Ok(response) = &mut completion.result {
                        // Never trust an on-the-wire copy of the internal marker.
                        response.headers.delete(CORS_REDIRECT_ORIGIN_HEADER);
                        if chain.cors_tainted
                            && chain
                                .request_policy
                                .as_ref()
                                .is_some_and(|policy| policy.mode == FetchRequestMode::Cors)
                        {
                            response
                                .headers
                                .insert_raw(CORS_REDIRECT_ORIGIN_HEADER, &chain.cors_origin);
                        }
                        response.redirected = chain.planner.followed() > 0;
                    }
                    visible.push(completion);
                }
                Err(error) => {
                    completion.result = Err(Self::redirect_error(error, &response_url));
                    visible.push(completion);
                }
                Ok(Some(mut next_request)) => {
                    if Self::is_preflight(&chain.current_request) {
                        completion.result = Err(FetchError::Blocked(
                            "CORS: redirected preflight responses are not supported".into(),
                        ));
                        visible.push(completion);
                        continue;
                    }

                    if chain.redirect_mode == FetchRedirectMode::Follow
                        && Self::current_cors_response_must_pass(&chain)
                    {
                        let credentialed = chain
                            .request_policy
                            .as_ref()
                            .is_some_and(|policy| policy.credentials == FetchCredentialsMode::Include);
                        let response = completion.result.as_ref().expect("checked above");
                        if let Err(error) = validate_cors_response_origin(
                            &chain.cors_origin,
                            credentialed,
                            response,
                        ) {
                            completion.result = Err(error);
                            visible.push(completion);
                            continue;
                        }
                    }

                    match chain.redirect_mode {
                        FetchRedirectMode::Error => {
                            completion.result = Err(FetchError::Blocked(format!(
                                "redirect mode \"error\" rejected redirect from {}",
                                response_url
                            )));
                            visible.push(completion);
                            continue;
                        }
                        FetchRedirectMode::Manual => {
                            if let Ok(response) = &mut completion.result {
                                response.redirected = true;
                            }
                            visible.push(completion);
                            continue;
                        }
                        FetchRedirectMode::Follow => {}
                    }

                    {
                        let response = completion.result.as_ref().expect("checked above");
                        chain.referrer.observe_redirect_response(response);
                    }

                    let effective_next = self.effective_url(&next_request);
                    let next_origin = Origin::of(&effective_next);
                    let mut redirect_preflight: Option<Vec<String>> = None;

                    match &chain.request_policy {
                        Some(policy) => {
                            let source_origin = Origin::of(&policy.source_url);
                            match policy.mode {
                                FetchRequestMode::SameOrigin => {
                                    if !source_origin.can_fetch(&effective_next) {
                                        completion.result = Err(FetchError::Blocked(format!(
                                            "{} may not follow redirect to {} in same-origin mode",
                                            source_origin.header_value(),
                                            effective_next
                                        )));
                                        visible.push(completion);
                                        continue;
                                    }
                                }
                                FetchRequestMode::NoCors => {
                                    // Preserve #188's conservative no-CORS redirect boundary.
                                    if next_origin != chain.initial_origin {
                                        completion.result = Err(FetchError::Blocked(format!(
                                            "no-cors redirect from {} to {} crossed origin",
                                            chain.initial_origin.header_value(),
                                            effective_next
                                        )));
                                        visible.push(completion);
                                        continue;
                                    }
                                }
                                FetchRequestMode::Cors => {
                                    let next_is_cross_origin =
                                        !source_origin.can_fetch(&effective_next);
                                    if next_is_cross_origin || chain.cors_tainted {
                                        let current_origin =
                                            Origin::of(&chain.current_request.url);
                                        if chain.cors_tainted && current_origin != next_origin {
                                            chain.cors_origin = "null".to_string();
                                        } else if !chain.cors_tainted {
                                            chain.cors_origin = source_origin.header_value();
                                        }
                                        chain.cors_tainted = true;
                                        next_request
                                            .headers
                                            .insert_raw("origin", &chain.cors_origin);

                                        let unsafe_headers =
                                            cors_unsafe_request_header_names(&next_request.headers);
                                        if !is_cors_safelisted_method(next_request.method)
                                            || !unsafe_headers.is_empty()
                                        {
                                            redirect_preflight = Some(unsafe_headers);
                                        }
                                    } else {
                                        next_request.headers.delete("origin");
                                    }
                                }
                            }
                        }
                        None => {
                            // Backward-compatible fallback for non-script callers that do not
                            // publish request mode: retain the old same-origin redirect guard.
                            if next_origin != chain.initial_origin {
                                completion.result = Err(FetchError::Blocked(format!(
                                    "redirect from {} to {} crossed origin",
                                    chain.initial_origin.header_value(),
                                    effective_next
                                )));
                                visible.push(completion);
                                continue;
                            }
                        }
                    }

                    let actual_cookie_policy =
                        Self::redirect_cookie_policy(&chain, &effective_next, next_request.method);
                    // Browser-owned Referer is added after CORS unsafe-header
                    // classification so it can never appear in
                    // Access-Control-Request-Headers.
                    Self::prepare_referrer_for_effective_target(
                        &chain.referrer,
                        &mut next_request,
                        &effective_next,
                    );

                    if let Some(requested_headers) = redirect_preflight {
                        let credentialed = chain.request_policy.as_ref().is_some_and(|policy| {
                            policy.credentials == FetchCredentialsMode::Include
                        });
                        let serialized_origin = chain.cors_origin.clone();
                        let requested_method = next_request.method;
                        let cached = preflight_cache_allows(
                            &self.cookie_jar,
                            self.now_ms(),
                            &serialized_origin,
                            &next_request.url,
                            credentialed,
                            requested_method,
                            &requested_headers,
                        );

                        if !cached {
                            let mut preflight_request = build_preflight_request(
                                next_request.url.clone(),
                                &serialized_origin,
                                requested_method,
                                &requested_headers,
                            );
                            // Fetch preflight inherits the actual request's
                            // referrer source/policy but its response must not
                            // mutate that redirect-chain policy.
                            Self::prepare_referrer_for_effective_target(
                                &chain.referrer,
                                &mut preflight_request,
                                &effective_next,
                            );
                            self.cookie_policies.set(id, preflight_cookie_policy());
                            chain.pending_preflight = Some(RedirectPreflight {
                                actual_request: next_request,
                                effective_url: effective_next,
                                actual_cookie_policy,
                                serialized_origin,
                                credentialed,
                                requested_method,
                                requested_headers,
                            });
                            chain.current_request = preflight_request.clone();
                            self.chains.borrow_mut().insert(id, chain);
                            self.inner.start(id, preflight_request);
                            continue;
                        }
                    }

                    self.cookie_policies.set(id, actual_cookie_policy);

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
        self.redirect_policies.remove(id);
        self.cors_redirect_policies.remove(id);
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
