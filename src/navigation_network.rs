// ============================================================
//  navigation_network.rs — synchronous top-level session policy
// ============================================================
//
// Browser navigation is intentionally synchronous today, while JavaScript
// Fetch uses the asynchronous NetworkBackend abstraction. This helper applies
// the same browser-owned HSTS and cookie state to a synchronous ResourceLoader
// request without changing Browser's public navigation API.

use std::rc::Rc;
use std::sync::Arc;

use crate::cookie_network::CookieJarRef;
use crate::cookie_same_site::SameSiteRequestContext;
use crate::eventloop::Clock;
use crate::hsts_network::HstsCacheRef;
use crate::net::{
    FetchError, FetchRequest, FetchResponse, LoadError, Method, ResourceLoader, Url,
};
use crate::redirect_policy::{RedirectError, RedirectPlanner};
use crate::referrer_policy::RedirectReferrerState;

/// Synchronous navigation-side companion to [`crate::session_network::SessionNetwork`].
///
/// Both layers can share the same CookieJar and HSTS cache. Each visible
/// top-level request — including every accepted redirect hop — is processed in
/// this order:
///
/// 1. apply learned HSTS to the outgoing URL;
/// 2. select cookies against that effective URL and the caller/chain SameSite
///    top-level context;
/// 3. compute browser-owned Referer state, when the caller supplied it;
/// 4. dispatch one [`ResourceLoader::fetch_once`] exchange;
/// 5. absorb response Set-Cookie fields;
/// 6. learn the first Strict-Transport-Security response field;
/// 7. update redirect Referrer-Policy, plan the next request, and repeat.
///
/// The caller owns initial request classification. Redirects then preserve a
/// conservative chain status: once any hop leaves the initial exact
/// scheme+host site, later hops remain cross-site for SameSite enforcement.
pub struct NavigationNetwork {
    loader: Arc<dyn ResourceLoader>,
    cookie_jar: CookieJarRef,
    hsts_cache: HstsCacheRef,
    clock: Rc<dyn Clock>,
}

impl NavigationNetwork {
    pub fn new(
        loader: Arc<dyn ResourceLoader>,
        cookie_jar: CookieJarRef,
        hsts_cache: HstsCacheRef,
        clock: Rc<dyn Clock>,
    ) -> NavigationNetwork {
        NavigationNetwork {
            loader,
            cookie_jar,
            hsts_cache,
            clock,
        }
    }

    pub fn cookie_jar(&self) -> CookieJarRef {
        self.cookie_jar.clone()
    }

    pub fn hsts_cache(&self) -> HstsCacheRef {
        self.hsts_cache.clone()
    }

    pub fn loader(&self) -> &Arc<dyn ResourceLoader> {
        &self.loader
    }

    fn now_ms(&self) -> u64 {
        self.clock.now_ms().max(0.0) as u64
    }

    /// Return the URL that top-level policy will actually dispatch after
    /// applying currently learned HSTS state.
    ///
    /// Browser-side SameSite classification must use this effective URL rather
    /// than the authored HTTP spelling. Otherwise an HTTPS document navigating
    /// to `http://same-host/...` can be misclassified as cross-site even though
    /// HSTS upgrades the request to HTTPS before cookie selection.
    pub fn effective_url(&self, url: &Url) -> Url {
        self.hsts_cache.borrow().upgrade_url(url, self.now_ms())
    }

    /// Load the first document in a fresh Browser session while retaining
    /// protocol response metadata and, when the loader opts in, following each
    /// redirect above Cookie/HSTS policy.
    ///
    /// The first request deliberately carries no Cookie header: a Browser
    /// session is created immediately before this call and therefore has no
    /// initiator/request-site cookie context. An intermediate response may,
    /// however, set a cookie or teach HSTS; those effects are absorbed before
    /// the next Location is dispatched.
    ///
    /// Arbitrary embedders remain source-compatible. If the loader does not
    /// advertise [`ResourceLoader::load_response_once`], this method falls back
    /// to the established `load_response()` behavior exactly as before.
    pub fn load_initial(&self, url: &Url) -> Result<FetchResponse, LoadError> {
        let effective_url = self.effective_url(url);
        let first_request = FetchRequest::get(effective_url.clone());

        let Some(first_response) = self.loader.load_response_once(&first_request)? else {
            let mut response = self.loader.load_response(&effective_url)?;
            self.absorb_response(&mut response);
            return Ok(response);
        };

        self.follow_initial_redirects(first_request, first_response)
    }

    fn follow_initial_redirects(
        &self,
        mut current: FetchRequest,
        mut response: FetchResponse,
    ) -> Result<FetchResponse, LoadError> {
        let mut planner = RedirectPlanner::default();
        let mut chain_same_site = true;

        loop {
            // Initial response state must become browser-visible before Location
            // is acted upon. A redirect-set cookie or STS rule can therefore
            // affect the immediately-following request.
            self.absorb_response(&mut response);

            let next = planner
                .next_request(&current, &response)
                .map_err(load_error_from_redirect)?;
            let Some(mut next) = next else {
                if !response.ok() {
                    return Err(LoadError::HttpStatus {
                        url: response.url.to_string(),
                        status: response.status,
                    });
                }
                if planner.followed() > 0 {
                    response.redirected = true;
                }
                return Ok(response);
            };

            let next_effective = self.effective_url(&next.url);
            chain_same_site = chain_same_site
                && conservative_same_site(&current.url, &next_effective);
            next.url = next_effective;

            let context = SameSiteRequestContext::new(chain_same_site, true, next.method);
            let effective = self.prepare_request(&next, context);
            let request_url = effective.url.clone();
            current = effective;

            response = match self.loader.load_response_once(&current)? {
                Some(response) => response,
                None => {
                    // Once a loader has opted into a one-hop chain, silently
                    // dropping back to an internally-following path would skip
                    // Cookie/HSTS policy on any later redirects. Fail closed
                    // instead of weakening the browser policy boundary.
                    return Err(LoadError::Io {
                        url: request_url.to_string(),
                        message: "loader stopped advertising single-hop document responses during redirect chain".to_string(),
                    });
                }
            };
        }
    }

    /// Perform a synchronous top-level request through browser session policy,
    /// following HTTP redirects above Cookie/HSTS policy rather than inside the
    /// transport.
    ///
    /// This compatibility entry point does not invent a referrer source from a
    /// wire header. Call [`NavigationNetwork::fetch_with_referrer`] when the
    /// document/navigation layer owns the source URL and policy state.
    pub fn fetch(
        &self,
        request: &FetchRequest,
        context: SameSiteRequestContext,
    ) -> Result<FetchResponse, FetchError> {
        self.fetch_with_referrer(request, context, None)
    }

    /// Perform a synchronous top-level request while carrying browser-owned
    /// referrer state through every redirect hop.
    ///
    /// The state keeps the original source URL separate from the serialized
    /// `Referer` header. An intermediate redirect response can therefore change
    /// `Referrer-Policy` and the next hop is recomputed from the stable source,
    /// rather than from a truncated header produced for the previous target.
    /// HSTS is applied before Referer computation so downgrade checks observe
    /// the URL that is actually dispatched.
    pub fn fetch_with_referrer(
        &self,
        request: &FetchRequest,
        context: SameSiteRequestContext,
        mut referrer: Option<RedirectReferrerState>,
    ) -> Result<FetchResponse, FetchError> {
        let mut planner = RedirectPlanner::default();
        let mut current = request.clone();
        let mut hop_context = SameSiteRequestContext::new(
            context.same_site,
            context.top_level_navigation,
            current.method,
        );

        loop {
            let mut effective = self.prepare_request(&current, hop_context);
            if let Some(state) = referrer.as_ref() {
                state.prepare_request(&mut effective);
            }
            let mut response = self.loader.fetch_once(&effective)?;

            // Response state becomes browser-visible before Location is acted
            // upon, so an intermediate Set-Cookie/STS field can influence the
            // very next hop.
            self.absorb_response(&mut response);

            let next = planner
                .next_request(&effective, &response)
                .map_err(fetch_error_from_redirect)?;
            let Some(mut next) = next else {
                if planner.followed() > 0 {
                    response.redirected = true;
                }
                return Ok(response);
            };

            // Fetch updates request referrer policy from the redirect response
            // before constructing the following hop. Only accepted redirects
            // update the chain; a final response's policy belongs to the new
            // document rather than to a request that will never be dispatched.
            if let Some(state) = referrer.as_mut() {
                state.observe_redirect_response(&response);
            }

            // SameSite is schemeful. Classify the URL after learned HSTS has
            // transformed it, matching Browser's first-hop classification.
            let next_effective = self.effective_url(&next.url);
            let chain_same_site = hop_context.same_site
                && conservative_same_site(&effective.url, &next_effective);
            next.url = next_effective;
            hop_context = SameSiteRequestContext::new(
                chain_same_site,
                hop_context.top_level_navigation,
                next.method,
            );
            current = next;
        }
    }

    /// Convenience GET used by ordinary navigation/reload/history loads.
    pub fn get(
        &self,
        url: &Url,
        context: SameSiteRequestContext,
    ) -> Result<FetchResponse, FetchError> {
        self.fetch(&FetchRequest::get(url.clone()), context)
    }

    /// Convenience GET for callers that own the document's referrer source and
    /// current policy state.
    pub fn get_with_referrer(
        &self,
        url: &Url,
        context: SameSiteRequestContext,
        referrer: RedirectReferrerState,
    ) -> Result<FetchResponse, FetchError> {
        self.fetch_with_referrer(&FetchRequest::get(url.clone()), context, Some(referrer))
    }

    fn prepare_request(
        &self,
        request: &FetchRequest,
        context: SameSiteRequestContext,
    ) -> FetchRequest {
        let mut effective = request.clone();
        effective.url = self.effective_url(&effective.url);

        if matches!(effective.url.scheme(), "http" | "https") {
            // Cookie is browser-owned. A caller-supplied value must not bypass
            // jar/domain/Secure/SameSite policy on a top-level request.
            effective.headers.delete("cookie");
            if let Some(value) = self.cookie_jar.borrow().get_http_cookie_header_for_context(
                &effective.url,
                self.now_ms(),
                SameSiteRequestContext::new(
                    context.same_site,
                    context.top_level_navigation,
                    effective.method,
                ),
            ) {
                effective.headers.insert_raw("cookie", &value);
            }
        }

        effective
    }

    fn absorb_response(&self, response: &mut FetchResponse) {
        let now_ms = self.now_ms();

        if matches!(response.url.scheme(), "http" | "https") {
            // Keep Set-Cookie fields separate; joining them with commas changes
            // cookie grammar and can corrupt Expires values.
            let set_cookies: Vec<String> = response
                .headers
                .iter()
                .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
                .map(|(_, value)| value.to_string())
                .collect();
            if !set_cookies.is_empty() {
                let mut jar = self.cookie_jar.borrow_mut();
                for value in set_cookies {
                    jar.store_set_cookie(&value, &response.url, now_ms);
                }
            }
        }

        // Set-Cookie is browser-owned state and must not leak past this policy
        // boundary, matching CookieNetwork's Fetch-visible behavior.
        response.headers.delete("set-cookie");

        // Match HstsNetwork: only the first STS field is processed, and
        // HstsCache itself rejects insecure/IP-literal sources.
        let first_sts = response
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("strict-transport-security"))
            .map(|(_, value)| value.to_string());
        if let Some(value) = first_sts {
            self.hsts_cache
                .borrow_mut()
                .observe_response(&response.url, &value, now_ms);
        }
    }
}

fn conservative_same_site(source: &Url, target: &Url) -> bool {
    source.scheme() == target.scheme() && source.host().eq_ignore_ascii_case(target.host())
}

fn fetch_error_from_redirect(error: RedirectError) -> FetchError {
    match error {
        RedirectError::InvalidLocation(location) => FetchError::InvalidUrl(location),
        RedirectError::UnsupportedScheme(scheme) => FetchError::UnsupportedScheme(scheme),
        RedirectError::TooManyRedirects(url) => FetchError::TooManyRedirects(url),
    }
}

fn load_error_from_redirect(error: RedirectError) -> LoadError {
    match error {
        RedirectError::InvalidLocation(location) => LoadError::InvalidUrl(location),
        RedirectError::UnsupportedScheme(scheme) => LoadError::UnsupportedScheme(scheme),
        RedirectError::TooManyRedirects(url) => LoadError::TooManyRedirects(url),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    use crate::cookie::CookieJar;
    use crate::eventloop::ManualClock;
    use crate::hsts::HstsCache;
    use crate::net::{HeaderMap, MemoryLoader};

    fn url(input: &str) -> Url {
        Url::parse(input).unwrap()
    }

    #[test]
    fn hsts_upgrade_happens_before_secure_cookie_selection() {
        let mut loader = MemoryLoader::new();
        loader.insert("https://example.test/page", "ok");
        let loader: Arc<dyn ResourceLoader> = Arc::new(loader);
        let clock = Rc::new(ManualClock::new());
        let jar = Rc::new(RefCell::new(CookieJar::with_clock(clock.clone())));
        jar.borrow_mut().store_set_cookie(
            "auth=secure; Path=/; Secure",
            &url("https://example.test/"),
            0,
        );
        let hsts = Rc::new(RefCell::new(HstsCache::new()));
        hsts.borrow_mut().observe_response(
            &url("https://example.test/"),
            "max-age=60",
            0,
        );

        let navigation = NavigationNetwork::new(loader, jar, hsts, clock);
        let response = navigation
            .get(
                &url("http://example.test/page"),
                SameSiteRequestContext::same_site(Method::Get),
            )
            .unwrap();

        assert_eq!(response.url.to_string(), "https://example.test/page");
    }

    #[test]
    fn effective_url_exposes_hsts_upgrade_for_request_classification() {
        let loader: Arc<dyn ResourceLoader> = Arc::new(MemoryLoader::new());
        let clock = Rc::new(ManualClock::new());
        let jar = Rc::new(RefCell::new(CookieJar::with_clock(clock.clone())));
        let hsts = Rc::new(RefCell::new(HstsCache::new()));
        hsts.borrow_mut().observe_response(
            &url("https://example.test/"),
            "max-age=60",
            0,
        );
        let navigation = NavigationNetwork::new(loader, jar, hsts, clock);

        assert_eq!(
            navigation
                .effective_url(&url("http://example.test/account"))
                .to_string(),
            "https://example.test/account"
        );
        assert_eq!(
            navigation
                .effective_url(&url("http://other.test/account"))
                .to_string(),
            "http://other.test/account"
        );
    }

    #[test]
    fn non_http_requests_do_not_receive_cookie_headers() {
        let mut loader = MemoryLoader::new();
        loader.insert("file:///tmp/page.html", "ok");
        let loader: Arc<dyn ResourceLoader> = Arc::new(loader);
        let clock = Rc::new(ManualClock::new());
        let jar = Rc::new(RefCell::new(CookieJar::with_clock(clock.clone())));
        let hsts = Rc::new(RefCell::new(HstsCache::new()));
        let navigation = NavigationNetwork::new(loader, jar, hsts, clock);

        let mut request = FetchRequest::get(url("file:///tmp/page.html"));
        request.headers = HeaderMap::new();
        assert!(navigation
            .fetch(
                &request,
                SameSiteRequestContext::same_site(Method::Get),
            )
            .is_ok());
    }
}
