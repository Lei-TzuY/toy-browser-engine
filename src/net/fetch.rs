// ============================================================
//  net/fetch.rs  —  The request/response model and the network layer
// ============================================================
//
//  This module knows about HTTP and nothing about JavaScript. It holds the
//  vocabulary every layer above shares — [`Method`], [`HeaderMap`],
//  [`FetchRequest`], [`FetchResponse`], [`FetchError`] — plus the two pieces
//  that make `fetch()` asynchronous:
//
//   • [`NetworkBackend`], which starts a request and later hands back a
//     [`FetchCompletion`]. It never runs a callback, touches the DOM or settles
//     a promise; it only produces data.
//   • [`FetchRegistry`], which remembers what is in flight. It is generic over
//     the handle the caller associates with a request (the script layer uses a
//     promise), so the bookkeeping can be tested with no runtime present.
//
//  Nothing here is allowed to block the browser thread on a socket: the
//  blocking client runs inside [`ThreadedNetwork`], on a worker that owns only
//  `Send` data.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use super::url::Url;
use super::ResourceLoader;

/// How many requests one page may have in flight at once.
///
/// A page looping over `fetch()` would otherwise create unbounded work; past
/// this point requests are rejected rather than queued, so the failure is
/// immediate and visible instead of a silent backlog.
pub const MAX_IN_FLIGHT_FETCHES: usize = 6;

/// Identifies one in-flight request.
pub type FetchId = u64;

// ── Methods ───────────────────────────────────────────────────────────────────

/// The request methods this engine can send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Head,
    Post,
    Put,
    Delete,
    Patch,
    Options,
}

impl Method {
    /// Parse a method name, normalising the known ones to upper case the way
    /// Fetch does. An unknown method is `None` rather than passed through.
    pub fn parse(input: &str) -> Option<Method> {
        match input.trim().to_ascii_uppercase().as_str() {
            "GET" => Some(Method::Get),
            "HEAD" => Some(Method::Head),
            "POST" => Some(Method::Post),
            "PUT" => Some(Method::Put),
            "DELETE" => Some(Method::Delete),
            "PATCH" => Some(Method::Patch),
            "OPTIONS" => Some(Method::Options),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Head => "HEAD",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
            Method::Patch => "PATCH",
            Method::Options => "OPTIONS",
        }
    }

    /// GET and HEAD carry no body; giving them one is an error.
    pub fn allows_body(&self) -> bool {
        !matches!(self, Method::Get | Method::Head)
    }

    /// HEAD responses have their body discarded, however much the server sent.
    pub fn wants_body(&self) -> bool {
        !matches!(self, Method::Head)
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── Headers ───────────────────────────────────────────────────────────────────

/// Headers that could not be used, kept separate from network failures because
/// they are the caller's mistake rather than the network's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderError(pub String);

impl fmt::Display for HeaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Headers a script may not set, because the engine owns them.
const FORBIDDEN_HEADERS: &[&str] = &[
    "host",
    "connection",
    "content-length",
    "transfer-encoding",
    "upgrade",
    "keep-alive",
];

/// An ordered, case-insensitive header list.
///
/// Names are stored normalised (trimmed, lower-cased) so `get("Content-Type")`
/// and `get("content-type")` are the same lookup, and duplicates are kept in
/// arrival order — `get` joins them with `", "` as Fetch specifies, while
/// `iter` still sees each line, which is what the wire format needs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeaderMap {
    entries: Vec<(String, String)>,
}

impl HeaderMap {
    pub fn new() -> HeaderMap {
        HeaderMap::default()
    }

    /// Normalise a header name, rejecting anything that cannot go on the wire.
    pub fn normalize_name(name: &str) -> Result<String, HeaderError> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(HeaderError("header name must not be empty".into()));
        }
        // RFC 7230 token characters, minus the separators.
        let valid = trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c));
        if !valid {
            return Err(HeaderError(format!("invalid header name: {trimmed:?}")));
        }
        Ok(trimmed.to_ascii_lowercase())
    }

    /// Trim a header value and refuse the newlines that enable header injection.
    pub fn normalize_value(value: &str) -> Result<String, HeaderError> {
        let trimmed = value.trim_matches(|c| c == ' ' || c == '\t');
        if trimmed.contains(['\r', '\n', '\0']) {
            return Err(HeaderError(
                "header value must not contain a newline".into(),
            ));
        }
        Ok(trimmed.to_string())
    }

    /// True for headers the engine sets itself, which a script may not override.
    pub fn is_forbidden(name: &str) -> bool {
        FORBIDDEN_HEADERS.contains(&name.to_ascii_lowercase().trim())
    }

    /// Replace every value for `name`.
    pub fn set(&mut self, name: &str, value: &str) -> Result<(), HeaderError> {
        let name = HeaderMap::normalize_name(name)?;
        let value = HeaderMap::normalize_value(value)?;
        self.entries.retain(|(existing, _)| existing != &name);
        self.entries.push((name, value));
        Ok(())
    }

    /// Add a value, keeping any already there.
    pub fn append(&mut self, name: &str, value: &str) -> Result<(), HeaderError> {
        let name = HeaderMap::normalize_name(name)?;
        let value = HeaderMap::normalize_value(value)?;
        self.entries.push((name, value));
        Ok(())
    }

    /// Set a header without validating it — for values the engine produces.
    pub fn insert_raw(&mut self, name: &str, value: &str) {
        let name = name.trim().to_ascii_lowercase();
        self.entries.retain(|(existing, _)| existing != &name);
        self.entries.push((name, value.trim().to_string()));
    }

    /// Add a header straight off the wire, keeping any already there.
    ///
    /// Unvalidated on purpose: a server's own headers arrive as bytes we
    /// parsed, not as something a script could use to forge a request.
    pub fn append_raw(&mut self, name: &str, value: &str) {
        self.entries
            .push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
    }

    /// Every value for `name`, joined with `", "`, or `None` if there are none.
    pub fn get(&self, name: &str) -> Option<String> {
        let name = name.trim().to_ascii_lowercase();
        let values: Vec<&str> = self
            .entries
            .iter()
            .filter(|(existing, _)| existing == &name)
            .map(|(_, value)| value.as_str())
            .collect();
        if values.is_empty() {
            None
        } else {
            Some(values.join(", "))
        }
    }

    pub fn has(&self, name: &str) -> bool {
        let name = name.trim().to_ascii_lowercase();
        self.entries.iter().any(|(existing, _)| existing == &name)
    }

    /// Remove every value for `name`. True if anything was there.
    pub fn delete(&mut self, name: &str) -> bool {
        let name = name.trim().to_ascii_lowercase();
        let before = self.entries.len();
        self.entries.retain(|(existing, _)| existing != &name);
        before != self.entries.len()
    }

    /// Distinct header names, in ascending order as Fetch requires.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.entries.iter().map(|(name, _)| name.clone()).collect();
        names.sort();
        names.dedup();
        names
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The MIME type from `Content-Type`, without its parameters.
    pub fn mime(&self) -> Option<String> {
        let value = self.get("content-type")?;
        Some(
            value
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase(),
        )
    }

    /// The `charset=` parameter of `Content-Type`, lower-cased.
    pub fn charset(&self) -> Option<String> {
        let value = self.get("content-type")?;
        value.split(';').skip(1).find_map(|parameter| {
            let (key, value) = parameter.split_once('=')?;
            if key.trim().eq_ignore_ascii_case("charset") {
                Some(value.trim().trim_matches('"').to_ascii_lowercase())
            } else {
                None
            }
        })
    }
}

// ── Origins ───────────────────────────────────────────────────────────────────

/// Who a document is, for the purposes of deciding what it may fetch.
///
/// Two shapes, because this engine loads pages from two very different places:
///
///  • `Tuple` is the web's origin — scheme, host and port must all match.
///  • `Local` covers `file:` and the in-memory demo scheme, which have no host.
///    A local document is confined to its own directory subtree, so a page
///    cannot walk up with `../` and read the rest of the disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Tuple {
        scheme: String,
        host: String,
        port: u16,
    },
    Local {
        scheme: String,
        /// The document's directory, with a trailing slash.
        directory: String,
    },
}

impl Origin {
    /// The origin of the document loaded from `url`.
    pub fn of(url: &Url) -> Origin {
        if url.host().is_empty() {
            let path = url.path();
            let directory = match path.rfind('/') {
                Some(index) => path[..=index].to_string(),
                None => "/".to_string(),
            };
            Origin::Local {
                scheme: url.scheme().to_string(),
                directory,
            }
        } else {
            Origin::Tuple {
                scheme: url.scheme().to_string(),
                host: url.host().to_ascii_lowercase(),
                port: url.port_or_default().unwrap_or(default_port(url.scheme())),
            }
        }
    }

    /// May a document with this origin fetch `target`?
    pub fn can_fetch(&self, target: &Url) -> bool {
        match (self, &Origin::of(target)) {
            (Origin::Tuple { .. }, other @ Origin::Tuple { .. }) => self == other,
            (
                Origin::Local {
                    scheme, directory, ..
                },
                Origin::Local {
                    scheme: other_scheme,
                    ..
                },
            ) => scheme == other_scheme && target.path().starts_with(directory.as_str()),
            // A page never crosses between the network and the local disk.
            _ => false,
        }
    }

    /// The `Origin` header value, or `null` for a local document.
    pub fn header_value(&self) -> String {
        match self {
            Origin::Tuple { scheme, host, port } => {
                if *port == default_port(scheme) {
                    format!("{scheme}://{host}")
                } else {
                    format!("{scheme}://{host}:{port}")
                }
            }
            Origin::Local { .. } => "null".to_string(),
        }
    }
}

fn default_port(scheme: &str) -> u16 {
    match scheme {
        "https" => 443,
        _ => 80,
    }
}

// ── Requests and responses ────────────────────────────────────────────────────

/// Everything the network needs to perform one request.
///
/// Deliberately plain data: it holds no handles, so it can be moved to a
/// worker thread without carrying any part of the DOM or the runtime with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchRequest {
    pub url: Url,
    pub method: Method,
    pub headers: HeaderMap,
    pub body: Option<Vec<u8>>,
}

impl FetchRequest {
    pub fn get(url: Url) -> FetchRequest {
        FetchRequest {
            url,
            method: Method::Get,
            headers: HeaderMap::new(),
            body: None,
        }
    }

    pub fn new(
        url: Url,
        method: Method,
        headers: HeaderMap,
        body: Option<Vec<u8>>,
    ) -> FetchRequest {
        FetchRequest {
            url,
            method,
            headers,
            body,
        }
    }
}

/// A completed response, whatever its status.
///
/// A 404 is a perfectly good `FetchResponse`: only a failure to *get* an answer
/// is a [`FetchError`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchResponse {
    /// The URL the response finally came from, after any redirects.
    pub url: Url,
    pub status: u16,
    pub status_text: String,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
    /// True when at least one redirect was followed.
    pub redirected: bool,
}

impl FetchResponse {
    /// A response built by the engine rather than parsed off a socket.
    pub fn synthetic(url: Url, status: u16, mime: Option<&str>, body: Vec<u8>) -> FetchResponse {
        let mut headers = HeaderMap::new();
        if let Some(mime) = mime {
            headers.insert_raw("content-type", mime);
        }
        headers.insert_raw("content-length", &body.len().to_string());
        FetchResponse {
            url,
            status,
            status_text: reason_phrase(status).to_string(),
            headers,
            body,
            redirected: false,
        }
    }

    /// The 2xx range, which is what `response.ok` reports.
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// The standard reason phrase for a status, used when a response is synthesised
/// or a server omitted one.
pub fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        410 => "Gone",
        413 => "Payload Too Large",
        418 => "I'm a teapot",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "",
    }
}

/// Why a request produced no response at all.
///
/// These, and only these, reject a `fetch()` promise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    InvalidUrl(String),
    UnsupportedScheme(String),
    UnsupportedMethod(String),
    /// The same-origin policy said no.
    Blocked(String),
    /// The request itself was malformed — a body on a GET, a bad header.
    BadRequest(String),
    Io(String),
    Timeout(String),
    MalformedResponse(String),
    TooManyRedirects(String),
    /// `AbortController::abort()`.
    Aborted,
    /// Past [`MAX_IN_FLIGHT_FETCHES`].
    TooManyRequests,
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FetchError::InvalidUrl(url) => write!(f, "TypeError: invalid URL: {url}"),
            FetchError::UnsupportedScheme(scheme) => match scheme.as_str() {
                "https" => write!(
                    f,
                    "TypeError: Failed to fetch: https is not supported \
                     (this engine speaks plain HTTP only, with no TLS stack)"
                ),
                other => write!(f, "TypeError: Failed to fetch: unsupported scheme {other}:"),
            },
            FetchError::UnsupportedMethod(method) => {
                write!(f, "TypeError: unsupported request method: {method}")
            }
            FetchError::Blocked(reason) => {
                write!(
                    f,
                    "TypeError: Failed to fetch: blocked by the same-origin policy ({reason})"
                )
            }
            FetchError::BadRequest(reason) => write!(f, "TypeError: {reason}"),
            FetchError::Io(message) => write!(f, "TypeError: Failed to fetch: {message}"),
            FetchError::Timeout(url) => write!(f, "TypeError: Failed to fetch: timed out at {url}"),
            FetchError::MalformedResponse(url) => {
                write!(
                    f,
                    "TypeError: Failed to fetch: malformed response from {url}"
                )
            }
            FetchError::TooManyRedirects(url) => {
                write!(f, "TypeError: Failed to fetch: too many redirects at {url}")
            }
            FetchError::Aborted => write!(f, "AbortError: the request was aborted"),
            FetchError::TooManyRequests => write!(
                f,
                "TypeError: Failed to fetch: too many requests in flight \
                 (at most {MAX_IN_FLIGHT_FETCHES} at a time)"
            ),
        }
    }
}

impl std::error::Error for FetchError {}

/// One finished request, on its way back to the event loop.
#[derive(Debug, Clone)]
pub struct FetchCompletion {
    pub id: FetchId,
    pub result: Result<FetchResponse, FetchError>,
}

// ── Backends ──────────────────────────────────────────────────────────────────

/// Performs requests. Implementations must never block the caller of `start`
/// on the network, and must never touch the DOM, the runtime or a promise —
/// they turn a [`FetchRequest`] into a [`FetchCompletion`] and nothing else.
pub trait NetworkBackend {
    /// Begin a request. Returns immediately.
    fn start(&self, id: FetchId, request: FetchRequest);

    /// Take every completion that has arrived since the last call.
    fn poll(&self) -> Vec<FetchCompletion>;

    /// Stop delivering `id`. The request may already be on the wire; what this
    /// guarantees is that its result never reaches the page.
    fn cancel(&self, _id: FetchId) {}

    /// True while a request is outstanding, so a driver knows to keep turning.
    fn is_busy(&self) -> bool {
        false
    }

    /// Block until an answer is ready, or `timeout` elapses. Returns true if
    /// something arrived.
    ///
    /// This is the one place a caller may wait on the network, and it is what
    /// an idle event loop does rather than spinning: it returns the instant
    /// data arrives, so it is a readiness wait and not a sleep. Backends with
    /// nothing to wait *for* — a manual one, an offline one — return false
    /// immediately.
    fn wait(&self, _timeout: Duration) -> bool {
        false
    }
}

/// A backend with no network behind it: every request fails.
///
/// This is what a `Document` has until something attaches a real one, so a
/// page built in isolation reports a clear error rather than pretending.
#[derive(Debug, Default)]
pub struct OfflineNetwork {
    ready: RefCell<Vec<FetchCompletion>>,
}

impl OfflineNetwork {
    pub fn new() -> OfflineNetwork {
        OfflineNetwork::default()
    }
}

impl NetworkBackend for OfflineNetwork {
    fn start(&self, id: FetchId, _request: FetchRequest) {
        self.ready.borrow_mut().push(FetchCompletion {
            id,
            result: Err(FetchError::Io("no network is attached to this page".into())),
        });
    }

    fn poll(&self) -> Vec<FetchCompletion> {
        std::mem::take(&mut *self.ready.borrow_mut())
    }

    fn is_busy(&self) -> bool {
        !self.ready.borrow().is_empty()
    }
}

/// Runs requests on the browser thread, during the network phase of the loop.
///
/// Suitable for sources that cannot block for long — the in-memory demo site
/// and the local filesystem. The work still happens outside any JavaScript
/// call stack, and the result is still delivered as a task, so the ordering a
/// page observes is the same as with a socket behind it.
pub struct LocalNetwork {
    loader: Arc<dyn ResourceLoader>,
    ready: RefCell<Vec<FetchCompletion>>,
    cancelled: RefCell<HashSet<FetchId>>,
}

impl LocalNetwork {
    pub fn new(loader: Arc<dyn ResourceLoader>) -> LocalNetwork {
        LocalNetwork {
            loader,
            ready: RefCell::new(Vec::new()),
            cancelled: RefCell::new(HashSet::new()),
        }
    }
}

impl NetworkBackend for LocalNetwork {
    fn start(&self, id: FetchId, request: FetchRequest) {
        let result = self.loader.fetch(&request);
        self.ready.borrow_mut().push(FetchCompletion { id, result });
    }

    fn poll(&self) -> Vec<FetchCompletion> {
        let ready = std::mem::take(&mut *self.ready.borrow_mut());
        let mut cancelled = self.cancelled.borrow_mut();
        ready
            .into_iter()
            .filter(|completion| !cancelled.remove(&completion.id))
            .collect()
    }

    fn cancel(&self, id: FetchId) {
        self.cancelled.borrow_mut().insert(id);
    }

    fn is_busy(&self) -> bool {
        !self.ready.borrow().is_empty()
    }
}

/// Runs blocking requests on worker threads.
///
/// This is what keeps a real HTTP fetch off the browser thread. A worker owns
/// an `Arc<dyn ResourceLoader>` and a `FetchRequest` — both `Send`, neither
/// connected to the DOM, the runtime or a promise — and its only output is a
/// [`FetchCompletion`] pushed down a channel. The `Send` bound is what enforces
/// that: nothing single-threaded can cross into the worker.
pub struct ThreadedNetwork {
    loader: Arc<dyn ResourceLoader>,
    sender: Sender<FetchCompletion>,
    receiver: Receiver<FetchCompletion>,
    /// Completions taken off the channel by `wait`, waiting for a `poll`.
    ready: RefCell<Vec<FetchCompletion>>,
    cancelled: RefCell<HashSet<FetchId>>,
    workers: RefCell<Vec<JoinHandle<()>>>,
}

impl ThreadedNetwork {
    pub fn new(loader: Arc<dyn ResourceLoader>) -> ThreadedNetwork {
        let (sender, receiver) = channel();
        ThreadedNetwork {
            loader,
            sender,
            receiver,
            ready: RefCell::new(Vec::new()),
            cancelled: RefCell::new(HashSet::new()),
            workers: RefCell::new(Vec::new()),
        }
    }

    /// Forget the workers that have finished, so handles do not pile up.
    fn reap(&self) {
        self.workers
            .borrow_mut()
            .retain(|handle| !handle.is_finished());
    }
}

impl NetworkBackend for ThreadedNetwork {
    fn start(&self, id: FetchId, request: FetchRequest) {
        let loader = self.loader.clone();
        let sender = self.sender.clone();
        let worker = std::thread::spawn(move || {
            let result = loader.fetch(&request);
            // The page may be gone by now; a closed channel is not an error.
            let _ = sender.send(FetchCompletion { id, result });
        });
        self.workers.borrow_mut().push(worker);
    }

    fn poll(&self) -> Vec<FetchCompletion> {
        self.reap();
        let mut arrived = std::mem::take(&mut *self.ready.borrow_mut());
        arrived.extend(self.receiver.try_iter());
        let mut cancelled = self.cancelled.borrow_mut();
        arrived
            .into_iter()
            .filter(|completion| !cancelled.remove(&completion.id))
            .collect()
    }

    fn cancel(&self, id: FetchId) {
        // The socket keeps going, but the answer is dropped on arrival.
        self.cancelled.borrow_mut().insert(id);
    }

    fn is_busy(&self) -> bool {
        self.reap();
        !self.workers.borrow().is_empty() || !self.ready.borrow().is_empty()
    }

    fn wait(&self, timeout: Duration) -> bool {
        match self.receiver.recv_timeout(timeout) {
            Ok(completion) => {
                self.ready.borrow_mut().push(completion);
                true
            }
            Err(_) => false,
        }
    }
}

/// Routes each request to the right backend, mirroring `DefaultLoader`.
///
/// Sockets are slow and go to a worker thread; the filesystem and the embedded
/// demo site are fast and stay on the browser thread.
pub struct DefaultNetwork {
    local: LocalNetwork,
    threaded: ThreadedNetwork,
}

impl DefaultNetwork {
    pub fn new(loader: Arc<dyn ResourceLoader>) -> DefaultNetwork {
        DefaultNetwork {
            local: LocalNetwork::new(loader.clone()),
            threaded: ThreadedNetwork::new(loader),
        }
    }
}

impl NetworkBackend for DefaultNetwork {
    fn start(&self, id: FetchId, request: FetchRequest) {
        match request.url.scheme() {
            "http" | "https" => self.threaded.start(id, request),
            _ => self.local.start(id, request),
        }
    }

    fn poll(&self) -> Vec<FetchCompletion> {
        let mut completions = self.local.poll();
        completions.extend(self.threaded.poll());
        completions
    }

    fn cancel(&self, id: FetchId) {
        self.local.cancel(id);
        self.threaded.cancel(id);
    }

    fn is_busy(&self) -> bool {
        self.local.is_busy() || self.threaded.is_busy()
    }

    fn wait(&self, timeout: Duration) -> bool {
        // Only the socket side can make a caller wait; the local side has
        // already finished whatever it was given.
        !self.local.is_busy() && self.threaded.wait(timeout)
    }
}

/// A network entirely under a test's control.
///
/// Nothing completes on its own: a test registers the answers it wants, starts
/// the page, asserts the promise is still pending, then completes requests by
/// hand. That makes every fetch test deterministic without a clock, a sleep or
/// a socket.
#[derive(Debug, Default)]
pub struct ManualNetwork {
    /// Canned answers, keyed by URL without its query or fragment.
    canned: RefCell<HashMap<String, Result<FetchResponse, FetchError>>>,
    /// Requests that have been started and not yet completed.
    started: RefCell<Vec<(FetchId, FetchRequest)>>,
    /// Completions waiting for the next `poll`.
    ready: RefCell<Vec<FetchCompletion>>,
    /// Every request ever started, for assertions about what was sent.
    seen: RefCell<Vec<FetchRequest>>,
    /// When set, a started request completes at once from the canned table.
    auto: Cell<bool>,
    cancelled: RefCell<HashSet<FetchId>>,
}

impl ManualNetwork {
    pub fn new() -> ManualNetwork {
        ManualNetwork::default()
    }

    fn key(url: &Url) -> String {
        url.without_query_and_fragment().to_string()
    }

    /// Answer `url` with `response`.
    pub fn respond(&self, url: &str, response: FetchResponse) {
        let key = Url::parse(url)
            .map(|parsed| ManualNetwork::key(&parsed))
            .unwrap_or_else(|_| url.to_string());
        self.canned.borrow_mut().insert(key, Ok(response));
    }

    /// Answer `url` with a body and a status.
    pub fn respond_with(&self, url: &str, status: u16, mime: &str, body: impl Into<Vec<u8>>) {
        let parsed = Url::parse(url).unwrap_or_else(|_| Url::from_file_path(url));
        self.respond(
            url,
            FetchResponse::synthetic(parsed, status, Some(mime), body.into()),
        );
    }

    /// Answer `url` with `200 OK` and a JSON body.
    pub fn respond_json(&self, url: &str, body: &str) {
        self.respond_with(url, 200, "application/json", body);
    }

    /// Answer `url` with `200 OK` and a plain-text body.
    pub fn respond_text(&self, url: &str, body: &str) {
        self.respond_with(url, 200, "text/plain; charset=utf-8", body);
    }

    /// Fail `url` at the network level, so the promise rejects.
    pub fn fail(&self, url: &str, error: FetchError) {
        let key = Url::parse(url)
            .map(|parsed| ManualNetwork::key(&parsed))
            .unwrap_or_else(|_| url.to_string());
        self.canned.borrow_mut().insert(key, Err(error));
    }

    /// Complete every started request as soon as it starts. The completion is
    /// still delivered as a task, so ordering is unchanged — only the waiting
    /// goes away.
    pub fn set_auto_complete(&self, auto: bool) {
        self.auto.set(auto);
    }

    /// Requests that have started and not yet been completed.
    pub fn pending(&self) -> Vec<(FetchId, String)> {
        self.started
            .borrow()
            .iter()
            .map(|(id, request)| (*id, request.url.to_string()))
            .collect()
    }

    pub fn pending_count(&self) -> usize {
        self.started.borrow().len()
    }

    /// Everything that was ever sent, in order.
    pub fn requests(&self) -> Vec<FetchRequest> {
        self.seen.borrow().clone()
    }

    /// The answer this network would give for a URL: the canned one, or a 404.
    fn answer_for(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        match self.canned.borrow().get(&ManualNetwork::key(&request.url)) {
            Some(Ok(response)) => {
                let mut response = response.clone();
                // HEAD keeps the metadata and drops the body, like a server.
                if !request.method.wants_body() {
                    response.body.clear();
                }
                Ok(response)
            }
            Some(Err(error)) => Err(error.clone()),
            None => Ok(FetchResponse::synthetic(
                request.url.clone(),
                404,
                Some("text/plain"),
                b"not found".to_vec(),
            )),
        }
    }

    fn finish(&self, id: FetchId, result: Result<FetchResponse, FetchError>) {
        self.started
            .borrow_mut()
            .retain(|(started, _)| *started != id);
        self.ready.borrow_mut().push(FetchCompletion { id, result });
    }

    /// Complete one started request from the canned table.
    pub fn complete(&self, id: FetchId) -> bool {
        let request = self
            .started
            .borrow()
            .iter()
            .find(|(started, _)| *started == id)
            .map(|(_, request)| request.clone());
        match request {
            Some(request) => {
                self.finish(id, self.answer_for(&request));
                true
            }
            None => false,
        }
    }

    /// Complete a started request with an answer chosen here and now.
    pub fn complete_with(&self, id: FetchId, result: Result<FetchResponse, FetchError>) -> bool {
        if !self
            .started
            .borrow()
            .iter()
            .any(|(started, _)| *started == id)
        {
            return false;
        }
        self.finish(id, result);
        true
    }

    /// Complete the first started request whose URL contains `needle`.
    pub fn complete_url(&self, needle: &str) -> bool {
        let found = self
            .started
            .borrow()
            .iter()
            .find(|(_, request)| request.url.to_string().contains(needle))
            .map(|(id, _)| *id);
        match found {
            Some(id) => self.complete(id),
            None => false,
        }
    }

    /// Complete everything outstanding, oldest first.
    pub fn complete_all(&self) -> usize {
        let ids: Vec<FetchId> = self.started.borrow().iter().map(|(id, _)| *id).collect();
        ids.iter().filter(|id| self.complete(**id)).count()
    }
}

impl NetworkBackend for ManualNetwork {
    fn start(&self, id: FetchId, request: FetchRequest) {
        self.seen.borrow_mut().push(request.clone());
        self.started.borrow_mut().push((id, request));
        if self.auto.get() {
            self.complete(id);
        }
    }

    fn poll(&self) -> Vec<FetchCompletion> {
        let ready = std::mem::take(&mut *self.ready.borrow_mut());
        let mut cancelled = self.cancelled.borrow_mut();
        ready
            .into_iter()
            .filter(|completion| !cancelled.remove(&completion.id))
            .collect()
    }

    fn cancel(&self, id: FetchId) {
        self.started
            .borrow_mut()
            .retain(|(started, _)| *started != id);
        self.cancelled.borrow_mut().insert(id);
    }

    fn is_busy(&self) -> bool {
        !self.started.borrow().is_empty() || !self.ready.borrow().is_empty()
    }
}

// ── The in-flight registry ────────────────────────────────────────────────────

/// What a page has in flight.
///
/// Generic over the handle a caller wants to keep alongside each request — the
/// script layer stores the pending promise there — so the bookkeeping can be
/// tested without a runtime, exactly like `Scheduler<T>`.
///
/// The registry is the authority on whether a completion is still wanted. A
/// completion whose id is not here is discarded, which is what makes a late
/// answer to the previous page harmless.
#[derive(Debug)]
pub struct FetchRegistry<T> {
    next_id: FetchId,
    pending: Vec<(FetchId, T)>,
    /// Requests waiting to be handed to a backend.
    outbox: Vec<(FetchId, FetchRequest)>,
    /// Requests whose answer is no longer wanted, waiting to be cancelled.
    cancelled: Vec<FetchId>,
    limit: usize,
}

impl<T> Default for FetchRegistry<T> {
    fn default() -> Self {
        FetchRegistry::new()
    }
}

impl<T> FetchRegistry<T> {
    pub fn new() -> FetchRegistry<T> {
        FetchRegistry {
            next_id: 1,
            pending: Vec::new(),
            outbox: Vec::new(),
            cancelled: Vec::new(),
            limit: MAX_IN_FLIGHT_FETCHES,
        }
    }

    pub fn with_limit(mut self, limit: usize) -> FetchRegistry<T> {
        self.limit = limit.max(1);
        self
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Record a request and queue it for the network.
    ///
    /// Fails when the page is already at [`MAX_IN_FLIGHT_FETCHES`], so a
    /// runaway loop is refused immediately instead of building a backlog.
    pub fn start(&mut self, request: FetchRequest, handle: T) -> Result<FetchId, FetchError> {
        if self.pending.len() >= self.limit {
            return Err(FetchError::TooManyRequests);
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.pending.push((id, handle));
        self.outbox.push((id, request));
        Ok(id)
    }

    /// Take the requests waiting to be sent.
    pub fn take_outbox(&mut self) -> Vec<(FetchId, FetchRequest)> {
        std::mem::take(&mut self.outbox)
    }

    /// Claim the handle for a finished request, if it is still wanted.
    pub fn take(&mut self, id: FetchId) -> Option<T> {
        let index = self
            .pending
            .iter()
            .position(|(pending, _)| *pending == id)?;
        Some(self.pending.remove(index).1)
    }

    /// Claim every handle matching a predicate — how an abort finds its
    /// request without the registry knowing what a signal is.
    pub fn take_where(&mut self, mut wanted: impl FnMut(&T) -> bool) -> Vec<(FetchId, T)> {
        let mut taken = Vec::new();
        let mut index = 0;
        while index < self.pending.len() {
            if wanted(&self.pending[index].1) {
                taken.push(self.pending.remove(index));
            } else {
                index += 1;
            }
        }
        // A request still in the outbox must not reach the network either, and
        // one already sent must have its answer dropped on arrival.
        let ids: Vec<FetchId> = taken.iter().map(|(id, _)| *id).collect();
        self.outbox.retain(|(id, _)| !ids.contains(id));
        self.cancelled.extend(ids);
        taken
    }

    /// Take the ids whose answers should no longer be delivered.
    pub fn take_cancellations(&mut self) -> Vec<FetchId> {
        std::mem::take(&mut self.cancelled)
    }

    pub fn contains(&self, id: FetchId) -> bool {
        self.pending.iter().any(|(pending, _)| *pending == id)
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// True while anything is in flight or waiting to be sent.
    pub fn has_pending_work(&self) -> bool {
        !self.pending.is_empty() || !self.outbox.is_empty()
    }

    /// Drop everything — what navigating away from a page does. The handles go
    /// with it, so no promise from the old page can ever be settled.
    pub fn clear(&mut self) -> Vec<FetchId> {
        let ids: Vec<FetchId> = self.pending.iter().map(|(id, _)| *id).collect();
        self.pending.clear();
        self.outbox.clear();
        self.cancelled.clear();
        ids
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).expect("valid URL")
    }

    // ── Methods ───────────────────────────────────────────────────────────

    #[test]
    fn methods_are_normalised_to_upper_case() {
        assert_eq!(Method::parse("get"), Some(Method::Get));
        assert_eq!(Method::parse("  Post "), Some(Method::Post));
        assert_eq!(Method::parse("DELETE"), Some(Method::Delete));
        assert_eq!(Method::parse("patch").map(|m| m.as_str()), Some("PATCH"));
    }

    #[test]
    fn unknown_methods_are_rejected_rather_than_passed_through() {
        assert_eq!(Method::parse("TRACE"), None);
        assert_eq!(Method::parse("CONNECT"), None);
        assert_eq!(Method::parse(""), None);
    }

    #[test]
    fn get_and_head_carry_no_body() {
        assert!(!Method::Get.allows_body());
        assert!(!Method::Head.allows_body());
        assert!(Method::Post.allows_body());
        assert!(!Method::Head.wants_body());
    }

    // ── Headers ───────────────────────────────────────────────────────────

    #[test]
    fn header_lookup_ignores_case() {
        let mut headers = HeaderMap::new();
        headers.set("Content-Type", "application/json").unwrap();
        assert_eq!(
            headers.get("content-type").as_deref(),
            Some("application/json")
        );
        assert_eq!(
            headers.get("CONTENT-TYPE").as_deref(),
            Some("application/json")
        );
        assert!(headers.has("Content-Type"));
    }

    #[test]
    fn set_replaces_and_append_accumulates() {
        let mut headers = HeaderMap::new();
        headers.append("accept", "text/html").unwrap();
        headers.append("Accept", "text/plain").unwrap();
        assert_eq!(
            headers.get("accept").as_deref(),
            Some("text/html, text/plain"),
            "duplicates are joined, in arrival order"
        );

        headers.set("accept", "*/*").unwrap();
        assert_eq!(headers.get("accept").as_deref(), Some("*/*"));
        assert_eq!(headers.len(), 1, "set replaced both values");
    }

    #[test]
    fn delete_removes_every_value() {
        let mut headers = HeaderMap::new();
        headers.append("x-tag", "a").unwrap();
        headers.append("x-tag", "b").unwrap();
        assert!(headers.delete("X-Tag"));
        assert!(!headers.has("x-tag"));
        assert!(!headers.delete("x-tag"), "deleting twice is harmless");
        assert_eq!(headers.get("x-tag"), None);
    }

    #[test]
    fn header_values_are_trimmed() {
        let mut headers = HeaderMap::new();
        headers.set("  X-Spaced  ", "  padded  ").unwrap();
        assert_eq!(headers.get("x-spaced").as_deref(), Some("padded"));
    }

    #[test]
    fn header_injection_is_refused() {
        let mut headers = HeaderMap::new();
        assert!(headers.set("X-Evil", "a\r\nX-Injected: yes").is_err());
        assert!(headers.set("Bad Name", "v").is_err());
        assert!(headers.set("", "v").is_err());
        assert!(headers.is_empty(), "nothing was stored");
    }

    #[test]
    fn engine_owned_headers_are_forbidden_to_scripts() {
        assert!(HeaderMap::is_forbidden("Host"));
        assert!(HeaderMap::is_forbidden("content-length"));
        assert!(!HeaderMap::is_forbidden("content-type"));
    }

    #[test]
    fn names_are_sorted_and_deduplicated() {
        let mut headers = HeaderMap::new();
        headers.append("b", "1").unwrap();
        headers.append("a", "2").unwrap();
        headers.append("b", "3").unwrap();
        assert_eq!(headers.names(), vec!["a", "b"]);
    }

    #[test]
    fn content_type_is_split_into_mime_and_charset() {
        let mut headers = HeaderMap::new();
        headers
            .set("content-type", "Text/HTML; charset=UTF-8")
            .unwrap();
        assert_eq!(headers.mime().as_deref(), Some("text/html"));
        assert_eq!(headers.charset().as_deref(), Some("utf-8"));

        let mut plain = HeaderMap::new();
        plain.set("content-type", "application/json").unwrap();
        assert_eq!(plain.charset(), None);
    }

    // ── Origins ───────────────────────────────────────────────────────────

    #[test]
    fn same_host_and_port_is_same_origin() {
        let origin = Origin::of(&url("http://example.com/page.html"));
        assert!(origin.can_fetch(&url("http://example.com/api/data.json")));
        assert!(origin.can_fetch(&url("http://example.com:80/other")));
    }

    #[test]
    fn a_different_host_port_or_scheme_is_cross_origin() {
        let origin = Origin::of(&url("http://example.com/page.html"));
        assert!(!origin.can_fetch(&url("http://other.example/data")));
        assert!(!origin.can_fetch(&url("http://example.com:8080/data")));
        assert!(!origin.can_fetch(&url("https://example.com/data")));
    }

    #[test]
    fn a_local_page_may_read_its_own_directory_only() {
        let origin = Origin::of(&url("demo:///site/index.html"));
        assert!(origin.can_fetch(&url("demo:///site/api/data.json")));
        assert!(
            !origin.can_fetch(&url("demo:///secrets.txt")),
            "a local page must not climb out of its directory"
        );
    }

    #[test]
    fn a_local_page_cannot_reach_the_network_and_back() {
        let local = Origin::of(&url("file:///home/user/page.html"));
        assert!(!local.can_fetch(&url("http://example.com/")));

        let remote = Origin::of(&url("http://example.com/page.html"));
        assert!(
            !remote.can_fetch(&url("file:///etc/passwd")),
            "a network page must never read the local disk"
        );
    }

    #[test]
    fn origin_header_values_omit_the_default_port() {
        assert_eq!(
            Origin::of(&url("http://example.com/x")).header_value(),
            "http://example.com"
        );
        assert_eq!(
            Origin::of(&url("http://example.com:8080/x")).header_value(),
            "http://example.com:8080"
        );
        assert_eq!(Origin::of(&url("demo:///x")).header_value(), "null");
    }

    // ── Responses ─────────────────────────────────────────────────────────

    #[test]
    fn ok_is_the_two_hundred_range() {
        let body = b"x".to_vec();
        for (status, expected) in [
            (200, true),
            (204, true),
            (299, true),
            (301, false),
            (404, false),
            (500, false),
        ] {
            let response = FetchResponse::synthetic(url("http://x/"), status, None, body.clone());
            assert_eq!(response.ok(), expected, "status {status}");
        }
    }

    #[test]
    fn synthetic_responses_carry_a_reason_phrase_and_length() {
        let response =
            FetchResponse::synthetic(url("http://x/a"), 404, Some("text/plain"), b"gone".to_vec());
        assert_eq!(response.status_text, "Not Found");
        assert_eq!(response.headers.get("content-length").as_deref(), Some("4"));
        assert_eq!(response.headers.mime().as_deref(), Some("text/plain"));
    }

    // ── The registry ──────────────────────────────────────────────────────

    #[test]
    fn starting_a_request_queues_it_and_remembers_its_handle() {
        let mut registry: FetchRegistry<&str> = FetchRegistry::new();
        let id = registry
            .start(FetchRequest::get(url("http://x/a")), "handle")
            .expect("started");

        assert!(registry.contains(id));
        assert_eq!(registry.len(), 1);
        let outbox = registry.take_outbox();
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox[0].0, id);
        assert!(
            registry.take_outbox().is_empty(),
            "the outbox is drained, not copied"
        );
    }

    #[test]
    fn a_completion_claims_its_handle_exactly_once() {
        let mut registry: FetchRegistry<&str> = FetchRegistry::new();
        let id = registry
            .start(FetchRequest::get(url("http://x/a")), "handle")
            .unwrap();

        assert_eq!(registry.take(id), Some("handle"));
        assert_eq!(registry.take(id), None, "a second completion is ignored");
        assert!(registry.is_empty());
    }

    #[test]
    fn an_unknown_completion_is_discarded() {
        let mut registry: FetchRegistry<&str> = FetchRegistry::new();
        assert_eq!(registry.take(999), None);
    }

    #[test]
    fn the_in_flight_limit_rejects_rather_than_queues() {
        let mut registry: FetchRegistry<u32> = FetchRegistry::new().with_limit(2);
        assert!(registry
            .start(FetchRequest::get(url("http://x/1")), 1)
            .is_ok());
        assert!(registry
            .start(FetchRequest::get(url("http://x/2")), 2)
            .is_ok());
        assert_eq!(
            registry.start(FetchRequest::get(url("http://x/3")), 3),
            Err(FetchError::TooManyRequests)
        );

        // Finishing one makes room again.
        registry.take(1);
        assert!(registry
            .start(FetchRequest::get(url("http://x/3")), 3)
            .is_ok());
    }

    #[test]
    fn take_where_claims_matching_handles_and_unsends_them() {
        let mut registry: FetchRegistry<&str> = FetchRegistry::new();
        registry
            .start(FetchRequest::get(url("http://x/a")), "keep")
            .unwrap();
        let doomed = registry
            .start(FetchRequest::get(url("http://x/b")), "abort")
            .unwrap();

        let taken = registry.take_where(|handle| *handle == "abort");
        assert_eq!(taken, vec![(doomed, "abort")]);
        assert_eq!(registry.len(), 1);
        assert_eq!(
            registry.take_outbox().len(),
            1,
            "the aborted request never reaches the network"
        );
        assert_eq!(
            registry.take_cancellations(),
            vec![doomed],
            "and one already sent has its answer dropped"
        );
    }

    #[test]
    fn clearing_the_registry_releases_every_handle() {
        use std::rc::Rc;

        let handle = Rc::new("page state");
        let mut registry: FetchRegistry<Rc<&str>> = FetchRegistry::new();
        registry
            .start(FetchRequest::get(url("http://x/a")), handle.clone())
            .unwrap();
        registry
            .start(FetchRequest::get(url("http://x/b")), handle.clone())
            .unwrap();
        assert_eq!(Rc::strong_count(&handle), 3);

        // What navigating away does.
        let ids = registry.clear();
        assert_eq!(ids.len(), 2);
        assert_eq!(
            Rc::strong_count(&handle),
            1,
            "a departed page must not keep its promises alive"
        );
        assert!(!registry.has_pending_work());
    }

    // ── Backends ──────────────────────────────────────────────────────────

    #[test]
    fn the_manual_network_holds_requests_until_told_to_complete() {
        let network = ManualNetwork::new();
        network.respond_json("http://x/api", r#"{"ok":true}"#);
        network.start(1, FetchRequest::get(url("http://x/api")));

        assert_eq!(network.pending_count(), 1);
        assert!(network.poll().is_empty(), "nothing completes on its own");

        assert!(network.complete(1));
        let completions = network.poll();
        assert_eq!(completions.len(), 1);
        let response = completions[0].result.as_ref().expect("a response");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, br#"{"ok":true}"#.to_vec());
        assert!(network.poll().is_empty(), "a completion is delivered once");
    }

    #[test]
    fn the_manual_network_answers_unregistered_urls_with_a_404() {
        let network = ManualNetwork::new();
        network.start(1, FetchRequest::get(url("http://x/missing")));
        network.complete_all();

        let completions = network.poll();
        let response = completions[0]
            .result
            .as_ref()
            .expect("a response, not an error");
        assert_eq!(response.status, 404);
        assert!(!response.ok());
    }

    #[test]
    fn the_manual_network_can_fail_at_the_network_level() {
        let network = ManualNetwork::new();
        network.fail("http://x/down", FetchError::Io("connection refused".into()));
        network.start(1, FetchRequest::get(url("http://x/down")));
        network.complete_all();

        assert!(network.poll()[0].result.is_err());
    }

    #[test]
    fn a_head_request_keeps_the_headers_and_drops_the_body() {
        let network = ManualNetwork::new();
        network.respond_text("http://x/doc", "a long document");
        let mut request = FetchRequest::get(url("http://x/doc"));
        request.method = Method::Head;
        network.start(1, request);
        network.complete_all();

        let completions = network.poll();
        let response = completions[0].result.as_ref().unwrap();
        assert_eq!(response.status, 200);
        assert!(response.body.is_empty());
        assert!(response.headers.has("content-type"));
    }

    #[test]
    fn cancelling_drops_a_completion_before_it_is_delivered() {
        let network = ManualNetwork::new();
        network.respond_text("http://x/a", "body");
        network.start(1, FetchRequest::get(url("http://x/a")));
        network.complete(1);
        network.cancel(1);
        assert!(network.poll().is_empty());
    }

    #[test]
    fn auto_complete_still_delivers_through_poll() {
        let network = ManualNetwork::new();
        network.respond_text("http://x/a", "body");
        network.set_auto_complete(true);
        network.start(1, FetchRequest::get(url("http://x/a")));

        // Even in auto mode, `start` produced no callback — only a completion
        // waiting to be collected.
        assert_eq!(network.pending_count(), 0);
        assert_eq!(network.poll().len(), 1);
    }

    #[test]
    fn the_manual_network_records_what_was_sent() {
        let network = ManualNetwork::new();
        let mut headers = HeaderMap::new();
        headers.set("content-type", "application/json").unwrap();
        network.start(
            1,
            FetchRequest::new(
                url("http://x/api"),
                Method::Post,
                headers,
                Some(b"{\"a\":1}".to_vec()),
            ),
        );

        let sent = network.requests();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].method, Method::Post);
        assert_eq!(
            sent[0].headers.get("content-type").as_deref(),
            Some("application/json")
        );
        assert_eq!(sent[0].body.as_deref(), Some(&b"{\"a\":1}"[..]));
    }

    #[test]
    fn the_offline_network_fails_every_request() {
        let network = OfflineNetwork::new();
        network.start(1, FetchRequest::get(url("http://x/a")));
        let completions = network.poll();
        assert!(matches!(completions[0].result, Err(FetchError::Io(_))));
    }
}
