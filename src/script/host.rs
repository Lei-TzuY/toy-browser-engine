// ============================================================
//  script/host.rs  —  Web-platform objects implemented in Rust
// ============================================================
//
//  `Headers`, `Request`, `Response`, `AbortController` and `AbortSignal` are
//  real objects a script holds, not strings with a convention attached. They
//  all arrive in the interpreter as one [`HostObject`] variant of `JsValue`,
//  so the value enum grows by a single arm however many of these the engine
//  gains.
//
//  The split from `net::fetch` is deliberate: that module is the wire format
//  and knows nothing about scripts; this one is the script's view of it and
//  adds exactly what JavaScript needs — shared mutable headers, single-use
//  bodies, and an abort flag two objects can see at once.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::fetch_redirect_policy::FetchRedirectMode;
use crate::net::fetch::{FetchRequest, FetchResponse, HeaderMap, Method};
use crate::referrer_policy::ReferrerPolicy;
use crate::net::Url;

/// A shared, mutable header list.
///
/// `response.headers` hands back a new wrapper each time, but every wrapper
/// points at the same map — so a `set` through one is visible through another,
/// the way a single JavaScript object would behave.
pub type HeadersRef = Rc<RefCell<HeaderMap>>;

pub fn headers_ref(headers: HeaderMap) -> HeadersRef {
    Rc::new(RefCell::new(headers))
}

// ── Bodies ────────────────────────────────────────────────────────────────────

/// A body that may be read exactly once.
///
/// Fetch calls this a stream, and reading it twice is an error there too. The
/// bytes are already in memory here, but the state machine is the same: the
/// first `text()` or `json()` takes them, and `bodyUsed` flips to true.
#[derive(Debug, Default)]
pub struct Body {
    bytes: RefCell<Option<Vec<u8>>>,
    used: Cell<bool>,
}

impl Body {
    pub fn new(bytes: Vec<u8>) -> Body {
        Body {
            bytes: RefCell::new(Some(bytes)),
            used: Cell::new(false),
        }
    }

    pub fn empty() -> Body {
        Body::new(Vec::new())
    }

    /// True once the body has been consumed.
    pub fn used(&self) -> bool {
        self.used.get()
    }

    /// Take the bytes, or explain why they are no longer there.
    pub fn take(&self) -> Result<Vec<u8>, String> {
        if self.used.get() {
            return Err("TypeError: body stream already read".to_string());
        }
        self.used.set(true);
        Ok(self.bytes.borrow_mut().take().unwrap_or_default())
    }

    /// Look at the transport-significant bytes without consuming them.
    ///
    /// The engine represents a body-less Request with the same empty Body
    /// wrapper used by the script API. Normalising zero-length bytes to `None`
    /// here prevents cloning a body-less GET Request from turning that wrapper
    /// into an authored GET body. Explicit `{ body: "" }` is still rejected
    /// before a Body is constructed, so this does not loosen Fetch validation.
    pub fn peek(&self) -> Option<Vec<u8>> {
        self.bytes
            .borrow()
            .clone()
            .filter(|bytes| !bytes.is_empty())
    }
}

/// Decode a body as text.
///
/// UTF-8 with a leading byte-order mark removed. Invalid sequences become the
/// replacement character rather than rejecting the promise, which is what a
/// browser does — a page that asked for text gets text.
pub fn decode_text(bytes: &[u8]) -> String {
    let without_bom = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8_lossy(without_bom).into_owned()
}

// ── Aborting ──────────────────────────────────────────────────────────────────

/// The flag an `AbortController` sets and its `AbortSignal` reports.
///
/// One allocation shared by both objects and by the fetch that is watching it,
/// so `controller.abort()` is visible everywhere at once.
#[derive(Debug, Default)]
pub struct AbortState {
    aborted: Cell<bool>,
}

impl AbortState {
    pub fn new() -> Rc<AbortState> {
        Rc::new(AbortState::default())
    }

    pub fn aborted(&self) -> bool {
        self.aborted.get()
    }

    /// Raise the flag. Returns true the first time, so a second `abort()` is a
    /// no-op rather than a second rejection.
    pub fn abort(&self) -> bool {
        !self.aborted.replace(true)
    }
}

// ── Requests ──────────────────────────────────────────────────────────────────

/// Fetch request mode carried by a script-visible `Request` object.
///
/// Keeping mode on RequestData is important: `new Request(existing)` and
/// `fetch(existing)` must not silently reconstruct a broader or narrower
/// cross-origin policy from the call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RequestMode {
    #[default]
    Cors,
    SameOrigin,
    NoCors,
}

impl RequestMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            RequestMode::Cors => "cors",
            RequestMode::SameOrigin => "same-origin",
            RequestMode::NoCors => "no-cors",
        }
    }
}

/// Cookie-credential mode carried by a script-visible `Request` object.
///
/// This state is browser-only metadata: it controls cookie send/store policy
/// and credentialed CORS validation but never becomes an authored wire header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RequestCredentials {
    #[default]
    SameOrigin,
    Omit,
    Include,
}

impl RequestCredentials {
    pub const fn as_str(self) -> &'static str {
        match self {
            RequestCredentials::SameOrigin => "same-origin",
            RequestCredentials::Omit => "omit",
            RequestCredentials::Include => "include",
        }
    }
}

/// Referrer source carried by a script-visible `Request`.
///
/// `Client` is the web-facing `about:client` sentinel: the actual source URL is
/// resolved from the environment only when Fetch is started. Keeping that
/// sentinel distinct from a concrete URL prevents a Request constructed under
/// one base URL from accidentally freezing that base as its referrer source.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RequestReferrer {
    #[default]
    Client,
    NoReferrer,
    Url(Url),
}

impl RequestReferrer {
    pub fn as_str(&self) -> String {
        match self {
            RequestReferrer::Client => "about:client".to_string(),
            RequestReferrer::NoReferrer => String::new(),
            RequestReferrer::Url(url) => url.without_fragment().to_string(),
        }
    }
}

/// A `Request` object.
#[derive(Debug)]
pub struct RequestData {
    pub url: Url,
    pub method: Method,
    pub headers: HeadersRef,
    pub body: Body,
    pub signal: Option<Rc<AbortState>>,
    pub mode: RequestMode,
    pub credentials: RequestCredentials,
    pub redirect: FetchRedirectMode,
    pub referrer: RequestReferrer,
    /// `None` is the script-visible empty-string policy and means inherit the
    /// environment's committed document policy when Fetch starts.
    pub referrer_policy: Option<ReferrerPolicy>,
}

impl RequestData {
    /// The wire request this describes.
    ///
    /// Reads the body without consuming it, so `fetch(request)` still leaves
    /// `request.bodyUsed` false until the script itself reads it. Mode and
    /// credentials remain browser-only metadata and therefore are intentionally
    /// absent from this transport representation.
    pub fn to_wire(&self) -> FetchRequest {
        FetchRequest::new(
            self.url.clone(),
            self.method,
            self.headers.borrow().clone(),
            self.body.peek(),
        )
    }
}

// ── Responses ─────────────────────────────────────────────────────────────────

/// Script-visible Fetch response type.
///
/// Synthetic `new Response()` values are `default`, ordinary same-origin
/// network responses are `basic`, successful CORS responses are `cors`, and
/// cross-origin `no-cors` responses are exposed only through an `opaque`
/// filtered view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResponseType {
    #[default]
    Default,
    Basic,
    Cors,
    Opaque,
    OpaqueRedirect,
}

impl ResponseType {
    pub const fn as_str(self) -> &'static str {
        match self {
            ResponseType::Default => "default",
            ResponseType::Basic => "basic",
            ResponseType::Cors => "cors",
            ResponseType::Opaque => "opaque",
            ResponseType::OpaqueRedirect => "opaqueredirect",
        }
    }
}

/// A `Response` object.
#[derive(Debug)]
pub struct ResponseData {
    pub url: Url,
    pub status: u16,
    pub status_text: String,
    pub headers: HeadersRef,
    pub body: Body,
    pub redirected: bool,
    pub response_type: ResponseType,
}

impl ResponseData {
    /// Wrap what came back from the network.
    pub fn from_wire(response: FetchResponse) -> ResponseData {
        ResponseData {
            url: response.url,
            status: response.status,
            status_text: response.status_text,
            headers: headers_ref(response.headers),
            body: Body::new(response.body),
            redirected: response.redirected,
            response_type: ResponseType::Basic,
        }
    }

    /// Build the script-visible opaque filtered view of a successful
    /// cross-origin no-CORS response. Cookie/HSTS processing has already seen
    /// the internal wire response before this wrapper is constructed.
    pub fn opaque_from_wire(response: FetchResponse) -> ResponseData {
        ResponseData {
            // Keep the internal URL only as non-script-visible bookkeeping. The
            // `url` getter below suppresses it for opaque responses.
            url: response.url,
            status: 0,
            status_text: String::new(),
            headers: headers_ref(HeaderMap::new()),
            body: Body::empty(),
            redirected: false,
            response_type: ResponseType::Opaque,
        }
    }

    /// Whether script sees a null-body opaque filtered response.
    pub fn is_opaque(&self) -> bool {
        matches!(
            self.response_type,
            ResponseType::Opaque | ResponseType::OpaqueRedirect
        )
    }

    /// Build the opaque-redirect filtered view returned by redirect=manual.
    pub fn opaque_redirect_from_wire(response: FetchResponse) -> ResponseData {
        ResponseData {
            url: response.url,
            status: 0,
            status_text: String::new(),
            headers: headers_ref(HeaderMap::new()),
            body: Body::empty(),
            redirected: false,
            response_type: ResponseType::OpaqueRedirect,
        }
    }

    pub fn script_url(&self) -> String {
        if self.is_opaque() {
            String::new()
        } else {
            self.url.to_string()
        }
    }

    pub fn body_used(&self) -> bool {
        !self.is_opaque() && self.body.used()
    }

    /// `response.ok` — the 2xx range, and nothing else.
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

// ── URL & URLSearchParams ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct UrlSearchParamsData {
    pub pairs: Rc<RefCell<Vec<(String, String)>>>,
    pub parent_url: Option<Rc<RefCell<UrlData>>>,
}

impl UrlSearchParamsData {
    pub fn new(pairs: Vec<(String, String)>, parent: Option<Rc<RefCell<UrlData>>>) -> Self {
        UrlSearchParamsData {
            pairs: Rc::new(RefCell::new(pairs)),
            parent_url: parent,
        }
    }

    pub fn from_query(query: &str, parent: Option<Rc<RefCell<UrlData>>>) -> Self {
        let pairs = parse_query_string(query);
        Self::new(pairs, parent)
    }

    pub fn sync_to_parent(&self) {
        if let Some(parent) = &self.parent_url {
            let qs = self.to_query_string();
            let mut u = parent.borrow_mut();
            if qs.is_empty() {
                u.url.set_query(None);
            } else {
                u.url.set_query(Some(qs));
            }
        }
    }

    pub fn to_query_string(&self) -> String {
        let pairs = self.pairs.borrow();
        pairs
            .iter()
            .map(|(k, v)| {
                format!(
                    "{}={}",
                    crate::net::url::percent_encode_query(k),
                    crate::net::url::percent_encode_query(v)
                )
            })
            .collect::<Vec<_>>()
            .join("&")
    }

    pub fn get(&self, name: &str) -> Option<String> {
        self.pairs
            .borrow()
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    }

    pub fn get_all(&self, name: &str) -> Vec<String> {
        self.pairs
            .borrow()
            .iter()
            .filter(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
            .collect()
    }

    pub fn has(&self, name: &str) -> bool {
        self.pairs.borrow().iter().any(|(k, _)| k == name)
    }

    pub fn set(&self, name: &str, value: &str) {
        let mut pairs = self.pairs.borrow_mut();
        let mut found = false;
        pairs.retain_mut(|(k, v)| {
            if k == name {
                if !found {
                    *v = value.to_string();
                    found = true;
                    true
                } else {
                    false
                }
            } else {
                true
            }
        });
        if !found {
            pairs.push((name.to_string(), value.to_string()));
        }
        drop(pairs);
        self.sync_to_parent();
    }

    pub fn append(&self, name: &str, value: &str) {
        self.pairs
            .borrow_mut()
            .push((name.to_string(), value.to_string()));
        self.sync_to_parent();
    }

    pub fn delete(&self, name: &str) {
        self.pairs.borrow_mut().retain(|(k, _)| k != name);
        self.sync_to_parent();
    }
}

pub fn parse_query_string(query: &str) -> Vec<(String, String)> {
    let clean = query.trim_start_matches('?');
    if clean.is_empty() {
        return Vec::new();
    }
    clean
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            if let Some(idx) = part.find('=') {
                let k = crate::net::url::percent_decode(&part[..idx].replace('+', " "));
                let v = crate::net::url::percent_decode(&part[idx + 1..].replace('+', " "));
                (k, v)
            } else {
                let k = crate::net::url::percent_decode(&part.replace('+', " "));
                (k, String::new())
            }
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct UrlData {
    pub url: Url,
}

impl UrlData {
    pub fn new(url: Url) -> Self {
        UrlData { url }
    }

    pub fn origin(&self) -> String {
        if let Some(port) = self.url.port() {
            format!("{}://{}:{}", self.url.scheme(), self.url.host(), port)
        } else {
            format!("{}://{}", self.url.scheme(), self.url.host())
        }
    }
}

// ── IntersectionObserver ────────────────────────────────────────────────────

/// An observed target inside the IntersectionObserver.
#[derive(Debug, Clone)]
pub struct IntersectionObserverTarget {
    pub element_id: String,
    pub is_intersecting: bool,
    pub intersection_ratio: f32,
}

/// Data backing a single IntersectionObserver instance.
#[derive(Debug, Clone)]
pub struct IntersectionObserverData {
    pub root: Option<String>,
    pub root_margin: String,
    pub thresholds: Vec<f32>,
    pub targets: Vec<IntersectionObserverTarget>,
}

impl IntersectionObserverData {
    pub fn new(thresholds: Vec<f32>) -> Self {
        Self {
            root: None,
            root_margin: "0px".to_string(),
            thresholds: if thresholds.is_empty() {
                vec![0.0]
            } else {
                thresholds
            },
            targets: Vec::new(),
        }
    }
}

/// An IntersectionObserverEntry snapshot.
#[derive(Debug, Clone)]
pub struct IntersectionObserverEntryData {
    pub target_id: String,
    pub is_intersecting: bool,
    pub intersection_ratio: f32,
    pub bounding_client_rect: [f32; 4], // [x, y, width, height]
    pub intersection_rect: [f32; 4],
    pub root_bounds: [f32; 4],
}

// ── ResizeObserver ──────────────────────────────────────────────────────────

/// Data backing a single ResizeObserver instance.
#[derive(Debug, Clone, Default)]
pub struct ResizeObserverData {
    pub targets: Vec<String>,
}

impl ResizeObserverData {
    pub fn new() -> Self {
        Self {
            targets: Vec::new(),
        }
    }
}

/// A ResizeObserverEntry snapshot.
#[derive(Debug, Clone)]
pub struct ResizeObserverEntryData {
    pub target_id: String,
    pub content_rect: [f32; 4],
    pub border_box_size: (f32, f32),
    pub content_box_size: (f32, f32),
}

// ── The value the interpreter sees ────────────────────────────────────────────

/// A Web-platform object.
///
/// One `JsValue` variant covers all of them, which is what keeps the value
/// enum from growing an arm per API. Each is behind an `Rc`, so passing one
/// around in script shares it rather than copying it.
#[derive(Debug)]
pub enum HostObject {
    Headers(HeadersRef),
    Request(RequestData),
    Response(ResponseData),
    AbortController(Rc<AbortState>),
    AbortSignal(Rc<AbortState>),
    CanvasRenderingContext2D(Rc<RefCell<crate::canvas::CanvasContext2D>>),
    URL(Rc<RefCell<UrlData>>),
    URLSearchParams(Rc<RefCell<UrlSearchParamsData>>),
    AudioContext(Rc<RefCell<crate::audio::AudioContext>>),
    AudioNode(Rc<RefCell<crate::audio::AudioContext>>, usize),
    AudioParam(Rc<RefCell<crate::audio::AudioContext>>, usize, String),
    IntersectionObserver(Rc<RefCell<IntersectionObserverData>>),
    IntersectionObserverEntry(IntersectionObserverEntryData),
    ResizeObserver(Rc<RefCell<ResizeObserverData>>),
    ResizeObserverEntry(ResizeObserverEntryData),
    JsMap(Rc<RefCell<Vec<(String, crate::script::interp::JsValue)>>>),
    JsSet(Rc<RefCell<Vec<String>>>),
    Crypto,
}

impl HostObject {
    /// The name a script sees in `String(value)`.
    pub fn type_name(&self) -> &'static str {
        match self {
            HostObject::Headers(_) => "Headers",
            HostObject::Request(_) => "Request",
            HostObject::Response(_) => "Response",
            HostObject::AbortController(_) => "AbortController",
            HostObject::AbortSignal(_) => "AbortSignal",
            HostObject::CanvasRenderingContext2D(_) => "CanvasRenderingContext2D",
            HostObject::URL(_) => "URL",
            HostObject::URLSearchParams(_) => "URLSearchParams",
            HostObject::AudioContext(_) => "AudioContext",
            HostObject::AudioNode(_, _) => "AudioNode",
            HostObject::AudioParam(_, _, _) => "AudioParam",
            HostObject::IntersectionObserver(_) => "IntersectionObserver",
            HostObject::IntersectionObserverEntry(_) => "IntersectionObserverEntry",
            HostObject::ResizeObserver(_) => "ResizeObserver",
            HostObject::ResizeObserverEntry(_) => "ResizeObserverEntry",
            HostObject::JsMap(_) => "Map",
            HostObject::JsSet(_) => "Set",
            HostObject::Crypto => "Crypto",
        }
    }

    pub fn as_headers(&self) -> Option<&HeadersRef> {
        match self {
            HostObject::Headers(headers) => Some(headers),
            _ => None,
        }
    }

    pub fn as_request(&self) -> Option<&RequestData> {
        match self {
            HostObject::Request(request) => Some(request),
            _ => None,
        }
    }

    pub fn as_response(&self) -> Option<&ResponseData> {
        match self {
            HostObject::Response(response) => Some(response),
            _ => None,
        }
    }

    /// The abort flag of a controller or a signal.
    pub fn as_abort_state(&self) -> Option<&Rc<AbortState>> {
        match self {
            HostObject::AbortController(state) | HostObject::AbortSignal(state) => Some(state),
            _ => None,
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_body_may_be_read_once() {
        let body = Body::new(b"hello".to_vec());
        assert!(!body.used());
        assert_eq!(body.take().unwrap(), b"hello".to_vec());
        assert!(body.used());

        let error = body.take().unwrap_err();
        assert!(error.contains("already read"), "{error}");
    }

    #[test]
    fn peeking_does_not_consume_a_body() {
        let body = Body::new(b"payload".to_vec());
        assert_eq!(body.peek(), Some(b"payload".to_vec()));
        assert!(!body.used(), "peeking is not reading");
        assert_eq!(body.take().unwrap(), b"payload".to_vec());
    }

    #[test]
    fn an_empty_body_is_still_a_body() {
        let body = Body::empty();
        assert_eq!(body.peek(), None, "an empty wrapper has no transport body");
        assert_eq!(body.take().unwrap(), Vec::<u8>::new());
        assert!(body.take().is_err(), "even an empty body is single-use");
    }

    #[test]
    fn text_decoding_strips_a_byte_order_mark() {
        assert_eq!(decode_text(&[0xEF, 0xBB, 0xBF, b'h', b'i']), "hi");
        assert_eq!(decode_text("héllo".as_bytes()), "héllo");
    }

    #[test]
    fn invalid_utf8_becomes_replacement_characters() {
        let decoded = decode_text(&[b'a', 0xFF, b'b']);
        assert!(decoded.starts_with('a') && decoded.ends_with('b'));
        assert!(decoded.contains('\u{FFFD}'), "{decoded:?}");
    }

    #[test]
    fn aborting_is_visible_through_every_handle() {
        let state = AbortState::new();
        let signal = state.clone();
        assert!(!signal.aborted());

        assert!(state.abort(), "the first abort takes effect");
        assert!(signal.aborted());
        assert!(!state.abort(), "a second abort changes nothing");
    }

    #[test]
    fn headers_are_shared_between_wrappers() {
        let shared = headers_ref(HeaderMap::new());
        let one = HostObject::Headers(shared.clone());
        let other = HostObject::Headers(shared.clone());

        one.as_headers()
            .unwrap()
            .borrow_mut()
            .set("x-tag", "value")
            .unwrap();
        assert_eq!(
            other.as_headers().unwrap().borrow().get("X-Tag").as_deref(),
            Some("value")
        );
    }

    #[test]
    fn a_request_converts_to_the_wire_without_consuming_its_body() {
        let request = RequestData {
            url: Url::parse("http://example.com/api").unwrap(),
            method: Method::Post,
            headers: headers_ref(HeaderMap::new()),
            body: Body::new(b"{}".to_vec()),
            signal: None,
            mode: RequestMode::SameOrigin,
            credentials: RequestCredentials::Omit,
            redirect: FetchRedirectMode::Follow,
            referrer: RequestReferrer::Client,
            referrer_policy: None,
        };

        let wire = request.to_wire();
        assert_eq!(wire.method, Method::Post);
        assert_eq!(wire.body.as_deref(), Some(&b"{}"[..]));
        assert!(!request.body.used(), "sending is not reading");
        assert_eq!(request.mode, RequestMode::SameOrigin);
        assert_eq!(request.credentials, RequestCredentials::Omit);
        assert_eq!(request.redirect, FetchRedirectMode::Follow);
        assert_eq!(request.referrer, RequestReferrer::Client);
        assert_eq!(request.referrer_policy, None);
    }

    #[test]
    fn request_policy_modes_have_stable_web_names() {
        assert_eq!(RequestMode::default().as_str(), "cors");
        assert_eq!(RequestMode::SameOrigin.as_str(), "same-origin");
        assert_eq!(RequestCredentials::default().as_str(), "same-origin");
        assert_eq!(RequestCredentials::Omit.as_str(), "omit");
        assert_eq!(RequestCredentials::Include.as_str(), "include");
        assert_eq!(FetchRedirectMode::default().as_str(), "follow");
        assert_eq!(FetchRedirectMode::Error.as_str(), "error");
        assert_eq!(FetchRedirectMode::Manual.as_str(), "manual");
        assert_eq!(RequestReferrer::Client.as_str(), "about:client");
        assert_eq!(RequestReferrer::NoReferrer.as_str(), "");
    }

    #[test]
    fn a_response_reports_ok_only_for_the_two_hundreds() {
        for (status, expected) in [(200, true), (204, true), (304, false), (404, false)] {
            let response = ResponseData::from_wire(FetchResponse::synthetic(
                Url::parse("http://x/").unwrap(),
                status,
                None,
                Vec::new(),
            ));
            assert_eq!(response.ok(), expected, "status {status}");
        }
    }

    #[test]
    fn host_objects_name_themselves() {
        assert_eq!(
            HostObject::Headers(headers_ref(HeaderMap::new())).type_name(),
            "Headers"
        );
        assert_eq!(
            HostObject::AbortSignal(AbortState::new()).type_name(),
            "AbortSignal"
        );
    }
}
