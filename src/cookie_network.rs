// ============================================================
//  cookie_network.rs — HTTP cookie policy around NetworkBackend
// ============================================================

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use crate::cookie::CookieJar;
use crate::eventloop::Clock;
use crate::net::{FetchCompletion, FetchId, FetchRequest, FetchResponse, NetworkBackend};

/// Shared cookie storage for one browser session.
pub type CookieJarRef = Rc<RefCell<CookieJar>>;

/// Adds RFC 6265 cookie send/store behavior around any network backend.
///
/// The wrapped backend remains responsible only for transport. This decorator
/// owns browser policy at the request/response boundary:
///
/// - outgoing HTTP(S) requests receive a jar-derived `Cookie` header;
/// - script-provided `Cookie` values are discarded rather than trusted;
/// - every `Set-Cookie` response line is parsed independently and stored;
/// - `Set-Cookie` is removed before the response reaches the script layer;
/// - cancellation, readiness and waiting semantics are delegated unchanged.
///
/// The same [`CookieJarRef`] can be shared with `document.cookie`, so script
/// writes and HTTP writes converge on one session state without teaching the
/// transport layer about JavaScript or DOM objects.
pub struct CookieNetwork {
    inner: Rc<dyn NetworkBackend>,
    jar: CookieJarRef,
    clock: Rc<dyn Clock>,
}

impl CookieNetwork {
    pub fn new(
        inner: Rc<dyn NetworkBackend>,
        jar: CookieJarRef,
        clock: Rc<dyn Clock>,
    ) -> CookieNetwork {
        CookieNetwork { inner, jar, clock }
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

    fn now_ms(&self) -> u64 {
        self.clock.now_ms().max(0.0) as u64
    }

    fn prepare_request(&self, mut request: FetchRequest) -> FetchRequest {
        if !matches!(request.url.scheme(), "http" | "https") {
            return request;
        }

        // `Cookie` is controlled by the browser jar, never by page-authored
        // request headers. This also prevents a stale Cookie header from a
        // reused request object from bypassing current expiration/scoping.
        request.headers.delete("cookie");
        if let Some(value) = self
            .jar
            .borrow()
            .get_http_cookie_header(&request.url, self.now_ms())
        {
            request.headers.insert_raw("cookie", &value);
        }
        request
    }

    fn absorb_response(&self, response: &mut FetchResponse) {
        if !matches!(response.url.scheme(), "http" | "https") {
            return;
        }

        // Keep individual Set-Cookie lines separate. HeaderMap::get() joins
        // duplicate headers with commas, which is not valid for Set-Cookie
        // because Expires-style attributes may themselves contain commas.
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
                if let Some(cookie) = CookieJar::parse_set_cookie(&value, &response.url, now_ms) {
                    jar.store(cookie, now_ms);
                }
            }
        }

        // Set-Cookie is a forbidden response-header name in Fetch. The network
        // policy layer consumes it, but page script must not observe it.
        response.headers.delete("set-cookie");
    }
}

impl NetworkBackend for CookieNetwork {
    fn start(&self, id: FetchId, request: FetchRequest) {
        self.inner.start(id, self.prepare_request(request));
    }

    fn poll(&self) -> Vec<FetchCompletion> {
        let mut completions = self.inner.poll();
        for completion in &mut completions {
            if let Ok(response) = &mut completion.result {
                self.absorb_response(response);
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
