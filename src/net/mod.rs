// ============================================================
//  net/mod.rs  —  Resource loading
// ============================================================
//
//  Everything the engine fetches — documents, stylesheets, scripts, images —
//  goes through a [`ResourceLoader`]. The loader is a trait so the rest of the
//  engine never talks to the filesystem or a socket directly: tests hand it a
//  [`MemoryLoader`], the CLI hands it a [`DefaultLoader`], and an embedder
//  could add caching or a TLS transport without touching the pipeline.

#[path = "fetch.rs"]
mod fetch_core;
mod readiness;

/// Public Fetch/network surface.
///
/// The protocol/runtime vocabulary comes directly from the private core, while
/// the worker-backed network types are exported through completion-aware
/// wrappers. Keeping the raw worker implementation private prevents callers of
/// either `net::ThreadedNetwork` or `net::fetch::ThreadedNetwork` from bypassing
/// the readiness invariant.
pub mod fetch {
    pub use super::fetch_core::{
        reason_phrase, FetchCompletion, FetchError, FetchId, FetchRegistry, FetchRequest,
        FetchResponse, HeaderError, HeaderMap, LocalNetwork, ManualNetwork, Method, NetworkBackend,
        OfflineNetwork, Origin, MAX_IN_FLIGHT_FETCHES,
    };
    pub use super::readiness::{DefaultNetwork, ThreadedNetwork};
}

pub mod http;
pub mod url;

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

pub use fetch::{
    DefaultNetwork, FetchCompletion, FetchError, FetchId, FetchRegistry, FetchRequest,
    FetchResponse, HeaderMap, LocalNetwork, ManualNetwork, Method, NetworkBackend, OfflineNetwork,
    Origin, ThreadedNetwork,
};
pub use http::HttpConfig;
pub use url::{Url, UrlError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    InvalidUrl(String),
    UnsupportedScheme(String),
    NotFound(String),
    HttpStatus { url: String, status: u16 },
    TooManyRedirects(String),
    Io { url: String, message: String },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::InvalidUrl(u) => write!(f, "invalid URL: {u}"),
            LoadError::UnsupportedScheme(scheme) => match scheme.as_str() {
                // Worth explaining: the engine has no TLS stack at all.
                "https" => write!(
                    f,
                    "https is not supported: this engine speaks plain HTTP only (no TLS stack)"
                ),
                other => write!(f, "unsupported scheme: {other}"),
            },
            LoadError::NotFound(p) => write!(f, "not found: {p}"),
            LoadError::HttpStatus { url, status } => write!(f, "HTTP {status} at {url}"),
            LoadError::TooManyRedirects(u) => write!(f, "too many redirects: {u}"),
            LoadError::Io { url, message } => write!(f, "IO error at {url}: {message}"),
        }
    }
}

impl std::error::Error for LoadError {}

/// Guess a MIME type from a path's extension.
pub fn mime_from_path(path: &str) -> &'static str {
    let extension = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match extension.as_str() {
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "txt" => "text/plain",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "ppm" => "image/x-portable-pixmap",
        _ => "application/octet-stream",
    }
}

/// A fetched resource.
#[derive(Debug, Clone)]
pub struct Resource {
    /// The URL the bytes actually came from (after redirects).
    pub url: Url,
    /// Lower-cased MIME type without parameters, when the source supplied one.
    pub mime: Option<String>,
    pub bytes: Vec<u8>,
}

impl Resource {
    pub fn new(url: Url, mime: Option<String>, bytes: Vec<u8>) -> Self {
        Self { url, mime, bytes }
    }

    /// The body decoded as UTF-8, replacing invalid sequences.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).to_string()
    }

    /// MIME type, falling back to a guess from the URL's file extension.
    pub fn effective_mime(&self) -> &str {
        self.mime
            .as_deref()
            .unwrap_or_else(|| mime_from_path(self.url.path()))
    }
}

/// Fetches bytes for a URL.
///
/// `Send + Sync` is not decoration: it is what lets a blocking loader be moved
/// onto a worker thread by [`ThreadedNetwork`], and what stops anything
/// single-threaded — a DOM node, a promise — from being smuggled along with it.
pub trait ResourceLoader: Send + Sync {
    fn load(&self, url: &Url) -> Result<Resource, LoadError>;

    /// Load one document-like resource while retaining response metadata when
    /// the source can provide it.
    ///
    /// The default deliberately goes through `load()`, not `fetch()`. Existing
    /// embedders therefore keep their exact load/error/side-effect semantics
    /// without implementing anything new. Protocol loaders may override this
    /// to preserve response headers such as Set-Cookie and STS for browser
    /// navigation policy while still reporting non-success statuses as the same
    /// `LoadError` variants that `load()` exposes.
    fn load_response(&self, url: &Url) -> Result<FetchResponse, LoadError> {
        Ok(response_from_resource(self.load(url)?))
    }

    /// Optionally perform one document-style request/response exchange without
    /// following redirects inside the loader.
    ///
    /// `Ok(None)` means this loader does not advertise a trustworthy one-hop
    /// document primitive. Callers must then keep using `load_response()` and
    /// its established compatibility/error semantics. This fail-closed default
    /// is deliberate: an arbitrary custom loader may follow redirects inside
    /// `load()` or `load_response()`, and treating that as one hop would skip
    /// browser Cookie/HSTS policy between responses.
    ///
    /// The full request is supplied so redirect policy can regenerate
    /// browser-owned Cookie/Referer headers and have those values reach the next
    /// wire hop. `Ok(Some(response))` means exactly one exchange was performed;
    /// a 3xx or non-success status remains response metadata so the higher
    /// navigation layer can process policy before deciding what to do next.
    fn load_response_once(
        &self,
        _request: &FetchRequest,
    ) -> Result<Option<FetchResponse>, LoadError> {
        Ok(None)
    }

    /// Perform a full request, the way `fetch()` needs it.
    ///
    /// The default serves `GET` and `HEAD` from [`load`](ResourceLoader::load)
    /// and behaves like a static file server: a hit is `200`, a miss is a `404`
    /// *response* rather than an error, and only a genuine failure to reach the
    /// source produces a [`FetchError`]. Loaders that speak a real protocol
    /// override this to send the method and body themselves.
    fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        static_fetch(|url| self.load(url), request)
    }

    /// Perform exactly one transport request when the source can expose that
    /// distinction. Redirect orchestration layers use this to observe a 3xx
    /// response before deciding whether and how to issue the next hop.
    ///
    /// The default deliberately delegates to `fetch()`. Existing custom
    /// loaders therefore preserve their current request semantics and are not
    /// required to implement a new method merely because the trait grew an
    /// additive capability. Protocol loaders with internal redirect handling
    /// can override this to expose their true single-hop primitive.
    fn fetch_once(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        self.fetch(request)
    }
}

fn response_from_resource(resource: Resource) -> FetchResponse {
    let mime = resource.effective_mime().to_string();
    FetchResponse::synthetic(resource.url, 200, Some(&mime), resource.bytes)
}

fn load_error_from_fetch(requested_url: &Url, error: FetchError) -> LoadError {
    match error {
        FetchError::InvalidUrl(text) => LoadError::InvalidUrl(text),
        FetchError::UnsupportedScheme(scheme) => LoadError::UnsupportedScheme(scheme),
        FetchError::TooManyRedirects(target) => LoadError::TooManyRedirects(target),
        other => LoadError::Io {
            url: requested_url.to_string(),
            message: other.to_string(),
        },
    }
}

fn single_hop_load_response_from_fetch(
    requested_url: &Url,
    result: Result<FetchResponse, FetchError>,
) -> Result<FetchResponse, LoadError> {
    result.map_err(|error| load_error_from_fetch(requested_url, error))
}

fn load_response_from_fetch(
    requested_url: &Url,
    result: Result<FetchResponse, FetchError>,
) -> Result<FetchResponse, LoadError> {
    let response = single_hop_load_response_from_fetch(requested_url, result)?;
    if response.ok() {
        Ok(response)
    } else {
        Err(LoadError::HttpStatus {
            url: response.url.to_string(),
            status: response.status,
        })
    }
}

/// The `fetch` behaviour of a source that only knows how to read a URL.
///
/// Shared by every loader that has no protocol of its own — the filesystem and
/// the in-memory site — so they all answer with the same status semantics.
pub fn static_fetch(
    load: impl Fn(&Url) -> Result<Resource, LoadError>,
    request: &FetchRequest,
) -> Result<FetchResponse, FetchError> {
    // A read-only source cannot accept a write. That is a `405`, not a network
    // failure: the request arrived and was answered, exactly as a static file
    // server would answer it, so the page can read the status and say so.
    if request.method.allows_body() {
        let mut response = FetchResponse::synthetic(
            request.url.clone(),
            405,
            Some("text/plain"),
            b"this source only serves reads".to_vec(),
        );
        response.headers.insert_raw("allow", "GET, HEAD");
        return Ok(response);
    }
    match load(&request.url) {
        Ok(resource) => {
            let mime = resource.effective_mime().to_string();
            let length = resource.bytes.len();
            let body = if request.method.wants_body() {
                resource.bytes
            } else {
                Vec::new()
            };
            let mut response = FetchResponse::synthetic(resource.url, 200, Some(&mime), body);
            // A HEAD still reports the size the body would have had.
            response
                .headers
                .insert_raw("content-length", &length.to_string());
            Ok(response)
        }
        // A missing file is what a static server answers with a 404 — a
        // response, so `fetch` resolves and the page can read `status`.
        Err(LoadError::NotFound(_)) => Ok(FetchResponse::synthetic(
            request.url.clone(),
            404,
            Some("text/plain"),
            b"not found".to_vec(),
        )),
        Err(LoadError::HttpStatus { url, status }) => Ok(FetchResponse::synthetic(
            Url::parse(&url).unwrap_or_else(|_| request.url.clone()),
            status,
            Some("text/plain"),
            Vec::new(),
        )),
        Err(LoadError::UnsupportedScheme(scheme)) => Err(FetchError::UnsupportedScheme(scheme)),
        Err(LoadError::InvalidUrl(text)) => Err(FetchError::InvalidUrl(text)),
        Err(LoadError::TooManyRedirects(text)) => Err(FetchError::TooManyRedirects(text)),
        Err(LoadError::Io { message, .. }) => Err(FetchError::Io(message)),
    }
}

// ── In-memory ─────────────────────────────────────────────────────────────────

/// Serves resources from a map — used for the built-in demo site and for
/// tests that must not touch the filesystem or the network.
pub struct MemoryLoader {
    resources: Mutex<HashMap<String, Vec<u8>>>,
}

impl MemoryLoader {
    pub fn new() -> Self {
        Self {
            resources: Mutex::new(HashMap::new()),
        }
    }

    /// Register bytes at `url`.
    ///
    /// Entries are keyed by scheme, host and path only: like a static file
    /// server, the same file answers `page.html`, `page.html#section` and
    /// `page.html?q=1`.
    pub fn insert(&mut self, url: impl AsRef<str>, content: impl Into<Vec<u8>>) {
        self.resources
            .lock()
            .unwrap()
            .insert(Self::key(url.as_ref()), content.into());
    }

    pub fn contains(&self, url: &Url) -> bool {
        self.resources
            .lock()
            .unwrap()
            .contains_key(&url.without_query_and_fragment().to_string())
    }

    pub fn is_empty(&self) -> bool {
        self.resources.lock().unwrap().is_empty()
    }

    fn key(url: &str) -> String {
        match Url::parse(url) {
            Ok(parsed) => parsed.without_query_and_fragment().to_string(),
            Err(_) => Url::from_file_path(url)
                .without_query_and_fragment()
                .to_string(),
        }
    }
}

impl Default for MemoryLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceLoader for MemoryLoader {
    fn load(&self, url: &Url) -> Result<Resource, LoadError> {
        let key = url.without_query_and_fragment().to_string();
        match self.resources.lock().unwrap().get(&key) {
            Some(bytes) => {
                let mime = mime_from_path(url.path()).to_string();
                // The requested URL is preserved, fragment included.
                Ok(Resource::new(url.clone(), Some(mime), bytes.clone()))
            }
            None => Err(LoadError::NotFound(url.to_string())),
        }
    }
}

// ── file: ─────────────────────────────────────────────────────────────────────

/// Reads `file:` URLs from the local filesystem.
pub struct FileLoader;

impl FileLoader {
    pub fn new() -> Self {
        Self
    }
}

impl Default for FileLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceLoader for FileLoader {
    fn load(&self, url: &Url) -> Result<Resource, LoadError> {
        if url.scheme() != "file" {
            return Err(LoadError::UnsupportedScheme(url.scheme().to_string()));
        }
        let path = url
            .to_file_path()
            .ok_or_else(|| LoadError::InvalidUrl(url.to_string()))?;
        match fs::read(&path) {
            Ok(bytes) => {
                let mime = Some(mime_from_path(&path.to_string_lossy()).to_string());
                Ok(Resource::new(url.clone(), mime, bytes))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(LoadError::NotFound(url.to_string()))
            }
            Err(e) => Err(LoadError::Io {
                url: url.to_string(),
                message: e.to_string(),
            }),
        }
    }
}

// ── http: ─────────────────────────────────────────────────────────────────────

/// Fetches `http:` URLs. `https:` is reported as unsupported rather than
/// silently downgraded.
#[derive(Default)]
pub struct HttpLoader {
    pub config: HttpConfig,
}

impl ResourceLoader for HttpLoader {
    fn load(&self, url: &Url) -> Result<Resource, LoadError> {
        match url.scheme() {
            "http" => http::get(url, &self.config),
            other => Err(LoadError::UnsupportedScheme(other.to_string())),
        }
    }

    fn load_response(&self, url: &Url) -> Result<FetchResponse, LoadError> {
        match url.scheme() {
            "http" => load_response_from_fetch(
                url,
                http::send(&FetchRequest::get(url.clone()), &self.config),
            ),
            other => Err(LoadError::UnsupportedScheme(other.to_string())),
        }
    }

    fn load_response_once(
        &self,
        request: &FetchRequest,
    ) -> Result<Option<FetchResponse>, LoadError> {
        match request.url.scheme() {
            "http" => single_hop_load_response_from_fetch(
                &request.url,
                http::send_once(request, &self.config),
            )
            .map(Some),
            other => Err(LoadError::UnsupportedScheme(other.to_string())),
        }
    }

    /// A real protocol, so every method and a request body go on the wire, and
    /// an error status comes back as a response.
    fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        match request.url.scheme() {
            "http" => http::send(request, &self.config),
            other => Err(FetchError::UnsupportedScheme(other.to_string())),
        }
    }

    fn fetch_once(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        match request.url.scheme() {
            "http" => http::send_once(request, &self.config),
            other => Err(FetchError::UnsupportedScheme(other.to_string())),
        }
    }
}

// ── Dispatching loader ────────────────────────────────────────────────────────

/// Routes each URL to the loader for its scheme.
pub struct DefaultLoader {
    /// Consulted first, so embedded resources can shadow the outside world.
    memory: MemoryLoader,
    file: FileLoader,
    http: HttpLoader,
}

impl DefaultLoader {
    pub fn new() -> Self {
        Self {
            memory: MemoryLoader::new(),
            file: FileLoader::new(),
            http: HttpLoader::default(),
        }
    }

    /// Add in-memory resources (the built-in demo site uses this).
    pub fn with_memory(mut self, memory: MemoryLoader) -> Self {
        self.memory = memory;
        self
    }

    pub fn with_http_config(mut self, config: HttpConfig) -> Self {
        self.http.config = config;
        self
    }
}

impl Default for DefaultLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceLoader for DefaultLoader {
    fn load(&self, url: &Url) -> Result<Resource, LoadError> {
        if self.memory.contains(url) {
            return self.memory.load(url);
        }
        match url.scheme() {
            "file" => self.file.load(url),
            // `https` reaches the HTTP loader so it can explain itself.
            "http" | "https" => self.http.load(url),
            // Any other scheme can only be served from embedded resources, so
            // a miss there is a missing file rather than an unknown protocol.
            _ => self.memory.load(url),
        }
    }

    fn load_response(&self, url: &Url) -> Result<FetchResponse, LoadError> {
        if self.memory.contains(url) {
            return self.memory.load_response(url);
        }
        match url.scheme() {
            "file" => self.file.load_response(url),
            "http" | "https" => self.http.load_response(url),
            _ => self.memory.load_response(url),
        }
    }

    fn load_response_once(
        &self,
        request: &FetchRequest,
    ) -> Result<Option<FetchResponse>, LoadError> {
        let requested_url = &request.url;
        let result = if self.memory.contains(requested_url) {
            self.memory.fetch_once(request)
        } else {
            match requested_url.scheme() {
                "file" => self.file.fetch_once(request),
                "http" | "https" => return self.http.load_response_once(request),
                _ => self.memory.fetch_once(request),
            }
        };
        single_hop_load_response_from_fetch(requested_url, result).map(Some)
    }

    /// Route the same way `load` does, so a `fetch()` and a navigation to one
    /// URL always reach the same source.
    fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        if self.memory.contains(&request.url) {
            return self.memory.fetch(request);
        }
        match request.url.scheme() {
            "file" => self.file.fetch(request),
            "http" | "https" => self.http.fetch(request),
            _ => self.memory.fetch(request),
        }
    }

    fn fetch_once(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        if self.memory.contains(&request.url) {
            return self.memory.fetch_once(request);
        }
        match request.url.scheme() {
            "file" => self.file.fetch_once(request),
            "http" | "https" => self.http.fetch_once(request),
            _ => self.memory.fetch_once(request),
        }
    }
}

/// Turn a CLI argument into a URL: absolute URLs are parsed, anything else is
/// treated as a filesystem path.
pub fn url_from_argument(arg: &str) -> Url {
    match Url::parse(arg) {
        Ok(url) => url,
        Err(_) => Url::from_file_path(Path::new(arg)),
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn file_loader_reads_a_file_and_guesses_its_type() {
        let dir = std::env::temp_dir().join("browser_engine_net_tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sheet.css");
        std::fs::write(&path, b"p { color: red; }").unwrap();

        let url = Url::from_file_path(&path);
        let resource = FileLoader.load(&url).expect("loaded");
        assert_eq!(resource.text(), "p { color: red; }");
        assert_eq!(resource.effective_mime(), "text/css");
    }

    #[test]
    fn file_loader_reports_missing_files() {
        let url = Url::from_file_path(std::env::temp_dir().join("definitely-not-here.html"));
        assert!(matches!(FileLoader.load(&url), Err(LoadError::NotFound(_))));
    }

    #[test]
    fn memory_loader_serves_registered_urls() {
        let mut memory = MemoryLoader::new();
        memory.insert("demo:///index.html", "<p>hi</p>");
        let url = Url::parse("demo:///index.html").unwrap();
        assert_eq!(memory.load(&url).unwrap().text(), "<p>hi</p>");
        assert_eq!(memory.load(&url).unwrap().effective_mime(), "text/html");

        let missing = Url::parse("demo:///nope.html").unwrap();
        assert!(matches!(memory.load(&missing), Err(LoadError::NotFound(_))));
    }

    #[test]
    fn memory_loader_ignores_fragments() {
        let mut memory = MemoryLoader::new();
        memory.insert("demo:///a.html", "x");
        let url = Url::parse("demo:///a.html#section").unwrap();
        assert!(memory.load(&url).is_ok());
        assert!(memory.contains(&url));
    }

    #[test]
    fn memory_loader_ignores_the_query_like_a_static_server() {
        let mut memory = MemoryLoader::new();
        memory.insert("demo:///search.html", "results");
        let url = Url::parse("demo:///search.html?q=toy+browser").unwrap();
        assert!(memory.contains(&url));
        assert_eq!(memory.load(&url).unwrap().text(), "results");
        // The resource still reports the URL that was requested.
        assert_eq!(
            memory.load(&url).unwrap().url.query(),
            Some("q=toy+browser")
        );
    }

    #[test]
    fn default_loader_rejects_https_with_an_explanation() {
        let url = Url::parse("https://example.com/").unwrap();
        let error = DefaultLoader::new().load(&url).unwrap_err();
        assert!(
            error.to_string().contains("https is not supported"),
            "{error}"
        );
    }

    #[test]
    fn default_loader_prefers_embedded_resources() {
        let mut memory = MemoryLoader::new();
        memory.insert("demo:///index.html", "embedded");
        let loader = DefaultLoader::new().with_memory(memory);
        let url = Url::parse("demo:///index.html").unwrap();
        assert_eq!(loader.load(&url).unwrap().text(), "embedded");
    }

    #[test]
    fn default_loader_reports_missing_embedded_files_as_not_found() {
        let mut memory = MemoryLoader::new();
        memory.insert("demo:///index.html", "embedded");
        let loader = DefaultLoader::new().with_memory(memory);
        let url = Url::parse("demo:///gone.png").unwrap();
        assert!(matches!(loader.load(&url), Err(LoadError::NotFound(_))));
    }

    #[test]
    fn mime_types_come_from_extensions() {
        assert_eq!(mime_from_path("/a/b/style.css"), "text/css");
        assert_eq!(mime_from_path("/a/logo.PNG"), "image/png");
        assert_eq!(mime_from_path("/a/photo.jpeg"), "image/jpeg");
        assert_eq!(mime_from_path("/a/unknown.xyz"), "application/octet-stream");
    }

    #[test]
    fn cli_arguments_become_urls() {
        assert_eq!(url_from_argument("http://example.com/x").scheme(), "http");
        assert_eq!(url_from_argument("page.html").scheme(), "file");
    }

    /// Serve one canned response on a loopback socket, then fetch it.
    #[test]
    fn http_loader_fetches_from_a_local_server() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buffer = [0u8; 1024];
            let _ = std::io::Read::read(&mut stream, &mut buffer);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 13\r\n\r\n<p>served</p>",
                )
                .unwrap();
        });

        let url = Url::parse(&format!("http://127.0.0.1:{port}/index.html")).unwrap();
        let resource = HttpLoader::default().load(&url).expect("fetched");
        assert_eq!(resource.text(), "<p>served</p>");
        assert_eq!(resource.effective_mime(), "text/html");
        server.join().unwrap();
    }

    #[test]
    fn http_loader_follows_a_redirect() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();

        let server = std::thread::spawn(move || {
            for response in [
                "HTTP/1.1 302 Found\r\nLocation: /final.html\r\nContent-Length: 0\r\n\r\n"
                    .to_string(),
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\nfinal".to_string(),
            ] {
                let (mut stream, _) = listener.accept().expect("accept");
                let mut buffer = [0u8; 1024];
                let _ = std::io::Read::read(&mut stream, &mut buffer);
                stream.write_all(response.as_bytes()).unwrap();
            }
        });

        let url = Url::parse(&format!("http://127.0.0.1:{port}/start.html")).unwrap();
        let resource = HttpLoader::default().load(&url).expect("fetched");
        assert_eq!(resource.text(), "final");
        assert_eq!(resource.url.path(), "/final.html");
        server.join().unwrap();
    }

    #[test]
    fn http_loader_surfaces_error_statuses() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buffer = [0u8; 1024];
            let _ = std::io::Read::read(&mut stream, &mut buffer);
            stream
                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
        });

        let url = Url::parse(&format!("http://127.0.0.1:{port}/missing.html")).unwrap();
        let error = HttpLoader::default().load(&url).unwrap_err();
        assert!(
            matches!(error, LoadError::HttpStatus { status: 404, .. }),
            "{error}"
        );
        server.join().unwrap();
    }
}
