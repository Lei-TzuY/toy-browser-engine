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

/// Synchronous navigation-side companion to [`crate::session_network::SessionNetwork`].
///
/// Both layers can share the same CookieJar and HSTS cache. The ordering is
/// deliberately identical to the asynchronous stack:
///
/// 1. apply learned HSTS to the outgoing URL;
/// 2. select cookies against that effective URL and the caller-supplied
///    SameSite/top-level context;
/// 3. dispatch through ResourceLoader::fetch;
/// 4. absorb response Set-Cookie fields;
/// 5. learn the first Strict-Transport-Security response field.
///
/// The caller owns request classification. This module does not guess whether
/// two URLs are same-site, which keeps public-suffix/site policy out of the
/// transport primitive.
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
    /// protocol response metadata.
    ///
    /// There is no initiator/request-site context or preexisting session cookie
    /// at this point, so this path deliberately does not synthesize a Cookie
    /// header. It does apply any supplied HSTS cache state and, crucially,
    /// absorbs Set-Cookie/STS before the returned HTML can run bootstrap script.
    /// `ResourceLoader::load_response` keeps legacy `load()` semantics for
    /// embedders that do not opt in to response metadata.
    pub fn load_initial(&self, url: &Url) -> Result<FetchResponse, LoadError> {
        let effective_url = self.effective_url(url);
        let mut response = self.loader.load_response(&effective_url)?;
        self.absorb_response(&mut response);
        Ok(response)
    }

    /// Perform one synchronous top-level request through browser session policy.
    pub fn fetch(
        &self,
        request: &FetchRequest,
        context: SameSiteRequestContext,
    ) -> Result<FetchResponse, FetchError> {
        let mut effective = request.clone();
        effective.url = self.effective_url(&effective.url);

        if matches!(effective.url.scheme(), "http" | "https") {
            // Cookie is browser-owned. A caller-supplied value must not bypass
            // jar/domain/Secure/SameSite policy on a top-level request.
            effective.headers.delete("cookie");
            if let Some(value) = self.cookie_jar.borrow().get_http_cookie_header_for_context(
                &effective.url,
                self.now_ms(),
                context,
            ) {
                effective.headers.insert_raw("cookie", &value);
            }
        }

        let mut response = self.loader.fetch(&effective)?;
        self.absorb_response(&mut response);
        Ok(response)
    }

    /// Convenience GET used by ordinary navigation/reload/history loads.
    pub fn get(
        &self,
        url: &Url,
        context: SameSiteRequestContext,
    ) -> Result<FetchResponse, FetchError> {
        self.fetch(&FetchRequest::get(url.clone()), context)
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
