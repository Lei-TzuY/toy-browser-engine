// ============================================================
//  net/http.rs  —  Minimal HTTP/1.1 client
// ============================================================
//
//  Enough of HTTP to fetch a page, its subresources and anything a script
//  asks for over a plain TCP socket: request line, headers, request bodies,
//  status parsing, redirects, and chunked transfer decoding. HTTPS is TLS
//  1.2/1.3 with rustls and verified certificate/host-name trust.
//
//  There is one request path. [`send`] is the whole client and speaks the
//  fetch vocabulary; [`get`] is navigation's view of the same code, mapping a
//  response back to a `Resource` and turning an error status into a
//  `LoadError` (because a page that 404s cannot be rendered, while a `fetch()`
//  that 404s is a perfectly good answer).

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

use super::fetch::{reason_phrase, FetchError, FetchRequest, FetchResponse, HeaderMap, Method};
use super::url::Url;
use super::{LoadError, Resource};

/// How many redirects to follow before giving up.
pub const DEFAULT_MAX_REDIRECTS: u8 = 5;
/// Connect/read timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
/// Ceiling on a single response body, so a hostile server cannot exhaust memory.
pub const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;
/// Ceiling on the header block, for the same reason.
pub const MAX_HEADER_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct HttpConfig {
    pub timeout: Duration,
    pub max_redirects: u8,
    pub user_agent: String,
    /// Additional application-trusted DER CA roots; empty means Mozilla roots only.
    /// Never supplied by a web page. Useful for private PKI and loopback tests.
    pub extra_tls_roots: Vec<Vec<u8>>,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            max_redirects: DEFAULT_MAX_REDIRECTS,
            user_agent: format!("BrowserEngineToy/{}", env!("CARGO_PKG_VERSION")),
            extra_tls_roots: Vec::new(),
        }
    }
}

/// One parsed HTTP response, before redirects are resolved.
#[derive(Debug)]
struct RawResponse {
    status: u16,
    status_text: String,
    headers: HeaderMap,
    body: Vec<u8>,
}

// ── The client ────────────────────────────────────────────────────────────────

/// Perform `request`, following redirects.
///
/// An error status is *not* an error: 404 and 500 come back as responses. Only
/// a failure to obtain an answer at all produces a [`FetchError`].
///
/// Redirects are deliberately conservative about browser-owned credentials.
/// The current transport still follows redirects below CookieNetwork, so it
/// cannot correctly re-select path/domain/SameSite cookies for the next hop.
/// Until redirect orchestration moves above browser policy, every redirected
/// hop drops Cookie rather than risking sending a cookie outside its scope.
pub fn send(request: &FetchRequest, config: &HttpConfig) -> Result<FetchResponse, FetchError> {
    let mut current = request.url.clone();
    let mut method = request.method;
    let mut body = request.body.clone();
    let mut headers = request.headers.clone();
    let mut redirected = false;

    for _ in 0..=config.max_redirects {
        let raw = exchange(&current, method, &headers, body.as_deref(), config)?;

        if let Some(location) = redirect_target(&raw) {
            let next = current
                .join(&location)
                .map_err(|e| FetchError::InvalidUrl(e.to_string()))?;
            // A page may not reach through a redirect what it could not fetch
            // directly, and the refusal comes before the next request is sent.
            if let Some(initiator) = &request.initiator {
                if !initiator.can_fetch(&next) {
                    return Err(FetchError::Blocked(format!(
                        "{} may not follow a redirect to {next}",
                        initiator.header_value()
                    )));
                }
            }

            // CookieNetwork selected Cookie for `current`, not `next`. Reusing
            // it even on a same-origin redirect can violate Path scoping, so
            // drop it on every hop until redirect policy can re-run per URL.
            headers.delete("cookie");

            // Credentials tied to one origin must never be forwarded to a new
            // origin merely because that origin appeared in Location.
            if !same_origin(&current, &next) {
                headers.delete("authorization");
                headers.delete("proxy-authorization");
            }

            if redirect_rewrites_to_get(raw.status, method) {
                method = Method::Get;
                body = None;
                remove_request_body_headers(&mut headers);
            }

            current = next;
            redirected = true;
            continue;
        }

        let body = if method.wants_body() {
            raw.body
        } else {
            Vec::new()
        };
        return Ok(FetchResponse {
            url: current,
            status: raw.status,
            status_text: raw.status_text,
            headers: raw.headers,
            body,
            redirected,
        });
    }
    Err(FetchError::TooManyRedirects(request.url.to_string()))
}

/// Perform exactly one HTTP request/response exchange.
///
/// Unlike [`send`], this does not inspect `Location` and never follows a
/// redirect. A 3xx response is returned exactly like any other response with
/// `redirected == false`, preserving its headers for a higher policy layer to
/// process. This is the transport primitive needed to move redirect
/// orchestration above HSTS/cookie policy without changing today's public
/// redirect-following API yet.
pub fn send_once(
    request: &FetchRequest,
    config: &HttpConfig,
) -> Result<FetchResponse, FetchError> {
    let raw = exchange(
        &request.url,
        request.method,
        &request.headers,
        request.body.as_deref(),
        config,
    )?;
    let body = if request.method.wants_body() {
        raw.body
    } else {
        Vec::new()
    };
    Ok(FetchResponse {
        url: request.url.clone(),
        status: raw.status,
        status_text: raw.status_text,
        headers: raw.headers,
        body,
        redirected: false,
    })
}

/// The `Location` of a Fetch redirect response, if this is one.
fn redirect_target(raw: &RawResponse) -> Option<String> {
    if !matches!(raw.status, 301 | 302 | 303 | 307 | 308) {
        return None;
    }
    raw.headers.get("location")
}

/// Fetch redirect method normalization.
///
/// 301/302 historically rewrite POST to GET; 303 rewrites every method except
/// GET/HEAD; 307/308 preserve method and body.
fn redirect_rewrites_to_get(status: u16, method: Method) -> bool {
    matches!(status, 301 | 302) && method == Method::Post
        || status == 303 && !matches!(method, Method::Get | Method::Head)
}

fn remove_request_body_headers(headers: &mut HeaderMap) {
    for name in [
        "content-encoding",
        "content-language",
        "content-location",
        "content-type",
    ] {
        headers.delete(name);
    }
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host().eq_ignore_ascii_case(right.host())
        && left.port_or_default() == right.port_or_default()
}

/// Fetch `url` over HTTP for navigation and subresources.
pub fn get(url: &Url, config: &HttpConfig) -> Result<Resource, LoadError> {
    let response = send(&FetchRequest::get(url.clone()), config).map_err(load_error_for(url))?;

    if !(200..300).contains(&response.status) {
        return Err(LoadError::HttpStatus {
            url: response.url.to_string(),
            status: response.status,
        });
    }
    Ok(Resource {
        url: response.url,
        mime: response.headers.mime(),
        bytes: response.body,
    })
}

/// Translate a fetch-level failure into the loader's error type.
fn load_error_for(url: &Url) -> impl Fn(FetchError) -> LoadError + '_ {
    move |error| match error {
        FetchError::UnsupportedScheme(scheme) => LoadError::UnsupportedScheme(scheme),
        FetchError::InvalidUrl(text) => LoadError::InvalidUrl(text),
        FetchError::TooManyRedirects(text) => LoadError::TooManyRedirects(text),
        other => LoadError::Io {
            url: url.to_string(),
            message: other.to_string(),
        },
    }
}

/// I/O is always through one transport: plain TCP for HTTP, verified TLS
/// for HTTPS. Browser policy (redirects, cookies, CORS) stays above this layer.
trait Transport: Read + Write {}
impl<T: Read + Write> Transport for T {}

fn tls_config(config: &HttpConfig) -> Result<Arc<ClientConfig>, FetchError> {
    static DEFAULT: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    if config.extra_tls_roots.is_empty() {
        return Ok(DEFAULT
            .get_or_init(|| {
                let roots = RootCertStore::from_iter(
                    webpki_roots::TLS_SERVER_ROOTS.iter().cloned(),
                );
                Arc::new(ClientConfig::builder()
                    .with_root_certificates(roots)
                    .with_no_client_auth())
            })
            .clone());
    }

    let mut roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    for der in &config.extra_tls_roots {
        roots.add(CertificateDer::from(der.clone()))
            .map_err(|e| FetchError::Io(format!("invalid additional TLS CA root: {e}")))?;
    }
    Ok(Arc::new(ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth()))
}

/// True only when a response with explicit HTTP/1.1 framing has arrived
/// completely. TLS peers can legitimately omit close_notify on socket close:
/// we never accept such an EOF unless the declared response is complete.
/// Parse response framing once; do not repeatedly rescan a growing body.
/// Close-delimited TLS requires a *clean* close_notify, not a raw socket EOF.
#[derive(Clone, Copy, Debug)]
enum BodyFraming {
    NoBody,
    ContentLength(usize),
    Chunked,
    CloseDelimited,
}

fn response_framing(head: &[u8], method: Method) -> Result<BodyFraming, ()> {
    let text = std::str::from_utf8(head).map_err(|_| ())?;
    let mut lines = text.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or(())?;
    if method == Method::Head || matches!(status, 204 | 304) || (100..200).contains(&status) {
        return Ok(BodyFraming::NoBody);
    }
    let mut length: Option<usize> = None;
    let mut has_transfer_encoding = false;
    let mut chunked = false;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("transfer-encoding") {
                has_transfer_encoding = true;
                chunked |= value.to_ascii_lowercase().contains("chunked");
            }
            if name.eq_ignore_ascii_case("content-length") {
                let n = value.trim().parse::<usize>().map_err(|_| ())?;
                if length.is_some_and(|previous| previous != n) {
                    return Err(());
                }
                length = Some(n);
            }
        }
    }
    if chunked {
        Ok(BodyFraming::Chunked)
    } else if has_transfer_encoding {
        // Unsupported transfer codings must not be mistaken for a
        // close-delimited unencoded response.
        Err(())
    } else if let Some(length) = length {
        Ok(BodyFraming::ContentLength(length))
    } else {
        Ok(BodyFraming::CloseDelimited)
    }
}

/// Incremental chunk scanner: each input byte is visited O(1) times,
/// regardless of TLS record and TCP read fragmentation.
#[derive(Clone, Copy, Debug)]
enum ChunkState {
    Size,
    Data(usize),
    DataEnd,
    Trailers,
    Done,
}
struct ChunkScanner {
    cursor: usize,
    line_start: usize,
    state: ChunkState,
}
impl ChunkScanner {
    fn new(start: usize) -> Self {
        Self { cursor: start, line_start: start, state: ChunkState::Size }
    }

    fn advance(&mut self, raw: &[u8]) -> Result<bool, ()> {
        loop {
            match self.state {
                ChunkState::Size | ChunkState::Trailers => {
                    // Keep the last byte if it could be the first half of CRLF.
                    let mut end = self.cursor;
                    while end + 1 < raw.len() && &raw[end..end + 2] != b"\r\n" {
                        end += 1;
                    }
                    if end + 1 >= raw.len() {
                        self.cursor = end;
                        return Ok(false);
                    }
                    let line = &raw[self.line_start..end];
                    self.cursor = end + 2;
                    self.line_start = self.cursor;
                    if matches!(self.state, ChunkState::Trailers) {
                        if line.is_empty() {
                            self.state = ChunkState::Done;
                            return Ok(true);
                        }
                        // Ignore trailer fields; their bytes remain bounded by
                        // the common wire-size cap and are never executed.
                    } else {
                        let header = std::str::from_utf8(line).map_err(|_| ())?;
                        let size = usize::from_str_radix(
                            header.split(';').next().unwrap_or("").trim(), 16,
                        ).map_err(|_| ())?;
                        self.state = if size == 0 {
                            ChunkState::Trailers
                        } else {
                            ChunkState::Data(size)
                        };
                    }
                }
                ChunkState::Data(remaining) => {
                    let available = raw.len() - self.cursor;
                    let consumed = available.min(remaining);
                    self.cursor += consumed;
                    let left = remaining - consumed;
                    if left != 0 {
                        self.state = ChunkState::Data(left);
                        return Ok(false);
                    }
                    self.state = ChunkState::DataEnd;
                }
                ChunkState::DataEnd => {
                    let Some(after_end) = self.cursor.checked_add(2) else {
                        return Err(());
                    };
                    if after_end > raw.len() {
                        return Ok(false);
                    }
                    if &raw[self.cursor..after_end] != b"\r\n" {
                        return Err(());
                    }
                    self.cursor = after_end;
                    self.line_start = self.cursor;
                    self.state = ChunkState::Size;
                }
                ChunkState::Done => return Ok(true),
            }
        }
    }
}

fn read_http_message(
    stream: &mut dyn Transport,
    url: &Url,
    method: Method,
    tls: bool,
) -> Result<Vec<u8>, FetchError> {
    let mut raw = Vec::new();
    let mut buf = [0u8; 8192];
    let mut framing: Option<BodyFraming> = None;
    let mut body_start = 0usize;
    let mut chunk: Option<ChunkScanner> = None;
    const MAX_WIRE_BYTES: usize = MAX_BODY_BYTES + MAX_HEADER_BYTES * 4;

    loop {
        if let Some(kind) = framing {
            let complete = match kind {
                BodyFraming::NoBody => true,
                BodyFraming::ContentLength(n) => raw.len() - body_start >= n,
                BodyFraming::Chunked => chunk
                    .as_mut()
                    .expect("chunk scanner created with framing")
                    .advance(&raw)
                    .map_err(|_| FetchError::MalformedResponse(url.to_string()))?,
                BodyFraming::CloseDelimited => false,
            };
            if complete {
                return Ok(raw);
            }
        }
        match stream.read(&mut buf) {
            Ok(0) => {
                // rustls returns Ok(0) after an authenticated close_notify.
                // Raw TCP also allows EOF framing; a TLS UnexpectedEof does not.
                if matches!(framing, Some(BodyFraming::CloseDelimited)) {
                    return Ok(raw);
                }
                if tls {
                    return Err(FetchError::Io(format!(
                        "{url}: TLS closed before a complete HTTP response"
                    )));
                }
                // Preserve plain-HTTP behavior for EOF, but reject malformed
                // headers and incomplete explicitly framed responses.
                return if framing.is_some() {
                    Err(FetchError::MalformedResponse(url.to_string()))
                } else {
                    Ok(raw)
                };
            }
            Ok(n) => {
                if raw.len().saturating_add(n) > MAX_WIRE_BYTES {
                    return Err(FetchError::Io(format!("{url}: response exceeds size limit")));
                }
                raw.extend_from_slice(&buf[..n]);
                if framing.is_none() {
                    if let Some(split) = find_subslice(&raw, b"\r\n\r\n") {
                        if split > MAX_HEADER_BYTES {
                            return Err(FetchError::MalformedResponse(url.to_string()));
                        }
                        let kind = response_framing(&raw[..split], method)
                            .map_err(|_| FetchError::MalformedResponse(url.to_string()))?;
                        body_start = split + 4;
                        if matches!(kind, BodyFraming::Chunked) {
                            chunk = Some(ChunkScanner::new(body_start));
                        }
                        framing = Some(kind);
                    } else if raw.len() > MAX_HEADER_BYTES {
                        return Err(FetchError::MalformedResponse(url.to_string()));
                    }
                }
            }
            Err(e) if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) => return Err(FetchError::Timeout(url.to_string())),
            // UnexpectedEof from rustls is an unclean transport close. It must
            // never finish a body without explicit length/chunk framing.
            Err(e) => return Err(FetchError::Io(format!("{url}: {e}"))),
        }
    }
}

/// One request/response exchange over a fresh connection.
fn exchange(
    url: &Url,
    method: Method,
    headers: &HeaderMap,
    body: Option<&[u8]>,
    config: &HttpConfig,
) -> Result<RawResponse, FetchError> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(FetchError::UnsupportedScheme(url.scheme().to_string()));
    }
    // URL authority stores an IPv6 host including brackets; address resolution
    // and ServerName both require the underlying literal without brackets.
    let host = url.host().trim_start_matches('[').trim_end_matches(']');
    let port = url.port_or_default().unwrap_or(80);
    if host.is_empty() {
        return Err(FetchError::InvalidUrl(format!("{url} has no host")));
    }

    let addresses = (host, port)
        .to_socket_addrs()
        .map_err(|e| FetchError::Io(format!("{url}: {e}")))?;
    let mut last_error = None;
    let mut tcp = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, config.timeout) {
            Ok(stream) => { tcp = Some(stream); break; }
            Err(e) => last_error = Some(e),
        }
    }
    let tcp = tcp.ok_or_else(|| FetchError::Io(format!("{url}: {}", last_error
        .map_or_else(|| "no address resolved".to_string(), |e| e.to_string()))))?;
    tcp.set_read_timeout(Some(config.timeout)).ok();
    tcp.set_write_timeout(Some(config.timeout)).ok();

    let is_tls = url.scheme() == "https";
    let mut stream: Box<dyn Transport> = if is_tls {
        let server_name = ServerName::try_from(host.to_string())
            .map_err(|_| FetchError::InvalidUrl(format!("{url}: invalid TLS server name")))?;
        let connection = ClientConnection::new(tls_config(config)?, server_name)
            .map_err(|e| FetchError::Io(format!("{url}: TLS configuration: {e}")))?;
        Box::new(StreamOwned::new(connection, tcp))
    } else {
        Box::new(tcp)
    };

    let head = request_head(url, method, headers, body, config);
    stream.write_all(head.as_bytes())
        .map_err(|e| FetchError::Io(format!("{url}: {e}")))?;
    if let Some(body) = body {
        stream.write_all(body)
            .map_err(|e| FetchError::Io(format!("{url}: {e}")))?;
    }
    stream.flush()
        .map_err(|e| FetchError::Io(format!("{url}: {e}")))?;

    let raw = read_http_message(&mut *stream, url, method, is_tls)?;
    parse_response(&raw).ok_or_else(|| FetchError::MalformedResponse(url.to_string()))
}

/// Build the request line and header block.
fn request_head(
    url: &Url,
    method: Method,
    headers: &HeaderMap,
    body: Option<&[u8]>,
    config: &HttpConfig,
) -> String {
    let default_port = if url.scheme() == "https" { 443 } else { 80 };
    let host_header = match url.port_or_default() {
        Some(port) if port != default_port => format!("{}:{}", url.host(), port),
        _ => url.host().to_string(),
    };

    let mut head = format!(
        "{method} {target} HTTP/1.1\r\n\
         Host: {host_header}\r\n",
        target = url.request_target(),
    );
    // The caller's headers first, then the ones the engine insists on.
    for (name, value) in headers.iter() {
        if HeaderMap::is_forbidden(name) {
            continue;
        }
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    if !headers.has("user-agent") {
        head.push_str(&format!("User-Agent: {}\r\n", config.user_agent));
    }
    if !headers.has("accept") {
        head.push_str("Accept: */*\r\n");
    }
    head.push_str("Accept-Encoding: identity\r\n");
    if let Some(body) = body {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str("Connection: close\r\n\r\n");
    head
}

fn parse_response(raw: &[u8]) -> Option<RawResponse> {
    let split = find_subslice(raw, b"\r\n\r\n")?;
    if split > MAX_HEADER_BYTES {
        return None;
    }
    let head = std::str::from_utf8(&raw[..split]).ok()?;
    let body = &raw[split + 4..];

    let mut lines = head.split("\r\n");
    let status_line = lines.next()?;
    let mut status_parts = status_line.split_whitespace();
    // "HTTP/1.1 404 Not Found" — version, code, then the reason phrase.
    let _version = status_parts.next()?;
    let status: u16 = status_parts.next()?.parse().ok()?;
    let reason: String = status_parts.collect::<Vec<_>>().join(" ");
    let status_text = if reason.is_empty() {
        reason_phrase(status).to_string()
    } else {
        reason
    };

    let mut headers = HeaderMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            // Straight from the wire: a server's own headers are not subject to
            // the checks that protect us from a script.
            headers.append_raw(name, value);
        }
    }

    let chunked = headers
        .get("transfer-encoding")
        .map(|value| value.to_ascii_lowercase().contains("chunked"))
        .unwrap_or(false);

    let mut body = if chunked {
        decode_chunked(body)
    } else {
        body.to_vec()
    };
    body.truncate(MAX_BODY_BYTES);

    Some(RawResponse {
        status,
        status_text,
        headers,
        body,
    })
}

/// Decode `Transfer-Encoding: chunked` bodies.
fn decode_chunked(mut input: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(line_end) = find_subslice(input, b"\r\n") {
        let header = std::str::from_utf8(&input[..line_end]).unwrap_or("");
        // A chunk header may carry extensions after `;`.
        let size_text = header.split(';').next().unwrap_or("").trim();
        let Ok(size) = usize::from_str_radix(size_text, 16) else {
            break;
        };
        if size == 0 {
            break;
        }
        let Some(start) = line_end.checked_add(2) else { break };
        let Some(end) = start.checked_add(size) else { break };
        let Some(after_chunk) = end.checked_add(2) else { break };
        if after_chunk > input.len() || input.get(end..after_chunk) != Some(b"\r\n") {
            break;
        }
        out.extend_from_slice(&input[start..end]);
        input = &input[after_chunk..];
    }
    out
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_fragmented_chunked_message_is_scanned_incrementally() {
        let mut body = Vec::new();
        for _ in 0..131_072 {
            body.extend_from_slice(b"1\r\nx\r\n");
        }
        body.extend_from_slice(b"0\r\n\r\n");
        let mut wire = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        wire.extend_from_slice(&body);
        let url = Url::parse("http://localhost/stream").unwrap();
        let raw = read_http_message(
            &mut std::io::Cursor::new(wire),
            &url,
            Method::Get,
            false,
        )
        .expect("many tiny chunks must be linear and valid");
        let response = parse_response(&raw).expect("decode complete chunked body");
        assert_eq!(response.body.len(), 131_072);
        assert!(response.body.iter().all(|byte| *byte == b'x'));
    }

    #[test]
    fn huge_chunk_size_never_overflows_or_panics() {
        let body = b"ffffffffffffffff\r\nx\r\n0\r\n\r\n";
        assert!(decode_chunked(body).is_empty());
        let mut wire = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        wire.extend_from_slice(body);
        let url = Url::parse("http://localhost/overflow").unwrap();
        assert!(read_http_message(
            &mut std::io::Cursor::new(wire),
            &url,
            Method::Get,
            false,
        ).is_err());
    }

    #[test]
    fn parses_status_headers_and_body() {
        let raw =
            b"HTTP/1.1 200 OK\r\nContent-Type: text/css; charset=utf-8\r\n\r\nbody { color: red }";
        let response = parse_response(raw).expect("parsed");
        assert_eq!(response.status, 200);
        assert_eq!(response.status_text, "OK");
        assert_eq!(
            response.headers.get("content-type").as_deref(),
            Some("text/css; charset=utf-8")
        );
        assert_eq!(response.body, b"body { color: red }");
    }

    #[test]
    fn keeps_a_multi_word_reason_phrase() {
        let raw = b"HTTP/1.1 404 Not Found\r\n\r\n";
        assert_eq!(parse_response(raw).unwrap().status_text, "Not Found");
    }

    #[test]
    fn supplies_a_reason_phrase_when_the_server_omits_one() {
        let raw = b"HTTP/1.1 204\r\n\r\n";
        let response = parse_response(raw).expect("parsed");
        assert_eq!(response.status, 204);
        assert_eq!(response.status_text, "No Content");
    }

    #[test]
    fn header_lookup_is_case_insensitive() {
        let raw = b"HTTP/1.1 404 Not Found\r\nCONTENT-TYPE: text/html\r\n\r\n";
        let response = parse_response(raw).expect("parsed");
        assert_eq!(response.status, 404);
        assert_eq!(
            response.headers.get("Content-Type").as_deref(),
            Some("text/html")
        );
    }

    #[test]
    fn repeated_headers_are_all_kept() {
        let raw = b"HTTP/1.1 200 OK\r\nX-Tag: a\r\nX-Tag: b\r\n\r\n";
        let response = parse_response(raw).expect("parsed");
        assert_eq!(response.headers.get("x-tag").as_deref(), Some("a, b"));
    }

    #[test]
    fn decodes_chunked_bodies() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let response = parse_response(raw).expect("parsed");
        assert_eq!(response.body, b"hello world");
    }

    #[test]
    fn malformed_responses_are_rejected() {
        assert!(parse_response(b"not http at all").is_none());
    }

    #[test]
    fn a_request_head_carries_the_method_host_and_body_length() {
        let url = Url::parse("http://example.com:8080/api?x=1").unwrap();
        let mut headers = HeaderMap::new();
        headers.set("content-type", "application/json").unwrap();
        let head = request_head(
            &url,
            Method::Post,
            &headers,
            Some(b"{}"),
            &HttpConfig::default(),
        );

        assert!(head.starts_with("POST /api?x=1 HTTP/1.1\r\n"), "{head}");
        assert!(head.contains("Host: example.com:8080\r\n"), "{head}");
        assert!(
            head.contains("content-type: application/json\r\n"),
            "{head}"
        );
        assert!(head.contains("Content-Length: 2\r\n"), "{head}");
        assert!(head.ends_with("\r\n\r\n"));
    }

    #[test]
    fn a_script_cannot_forge_the_headers_the_engine_owns() {
        let url = Url::parse("http://example.com/").unwrap();
        let mut headers = HeaderMap::new();
        headers.insert_raw("host", "evil.example");
        headers.insert_raw("content-length", "999");
        let head = request_head(&url, Method::Get, &headers, None, &HttpConfig::default());

        assert!(head.contains("Host: example.com\r\n"));
        assert!(!head.contains("evil.example"), "{head}");
        assert!(!head.contains("999"), "{head}");
    }

    #[test]
    fn a_caller_supplied_user_agent_replaces_the_default() {
        let url = Url::parse("http://example.com/").unwrap();
        let mut headers = HeaderMap::new();
        headers.set("user-agent", "Custom/1.0").unwrap();
        let head = request_head(&url, Method::Get, &headers, None, &HttpConfig::default());

        assert!(head.contains("user-agent: Custom/1.0\r\n"), "{head}");
        assert!(!head.contains("BrowserEngineToy"), "{head}");
    }

    #[test]
    fn only_fetch_redirect_statuses_follow_location() {
        for status in [301, 302, 303, 307, 308] {
            let mut headers = HeaderMap::new();
            headers.insert_raw("location", "/next");
            let raw = RawResponse {
                status,
                status_text: String::new(),
                headers,
                body: Vec::new(),
            };
            assert_eq!(redirect_target(&raw).as_deref(), Some("/next"));
        }
        for status in [300, 304, 305, 306, 399] {
            let mut headers = HeaderMap::new();
            headers.insert_raw("location", "/next");
            let raw = RawResponse {
                status,
                status_text: String::new(),
                headers,
                body: Vec::new(),
            };
            assert!(redirect_target(&raw).is_none(), "status {status}");
        }
    }

    #[test]
    fn redirect_method_rewrite_matches_fetch_rules() {
        assert!(redirect_rewrites_to_get(301, Method::Post));
        assert!(redirect_rewrites_to_get(302, Method::Post));
        assert!(!redirect_rewrites_to_get(301, Method::Put));
        assert!(!redirect_rewrites_to_get(302, Method::Patch));
        assert!(redirect_rewrites_to_get(303, Method::Post));
        assert!(redirect_rewrites_to_get(303, Method::Put));
        assert!(!redirect_rewrites_to_get(303, Method::Get));
        assert!(!redirect_rewrites_to_get(303, Method::Head));
        assert!(!redirect_rewrites_to_get(307, Method::Post));
        assert!(!redirect_rewrites_to_get(308, Method::Post));
    }
}
