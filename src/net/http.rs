// ============================================================
//  net/http.rs  —  Minimal HTTP/1.1 client
// ============================================================
//
//  Enough of HTTP to fetch a page, its subresources and anything a script
//  asks for over a plain TCP socket: request line, headers, request bodies,
//  status parsing, redirects, and chunked transfer decoding. No TLS — see
//  `net::LoadError::UnsupportedScheme`.
//
//  There is one request path. [`send`] is the whole client and speaks the
//  fetch vocabulary; [`get`] is navigation's view of the same code, mapping a
//  response back to a `Resource` and turning an error status into a
//  `LoadError` (because a page that 404s cannot be rendered, while a `fetch()`
//  that 404s is a perfectly good answer).

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

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
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            max_redirects: DEFAULT_MAX_REDIRECTS,
            user_agent: format!("BrowserEngineToy/{}", env!("CARGO_PKG_VERSION")),
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

/// One request/response exchange over a fresh connection.
fn exchange(
    url: &Url,
    method: Method,
    headers: &HeaderMap,
    body: Option<&[u8]>,
    config: &HttpConfig,
) -> Result<RawResponse, FetchError> {
    if url.scheme() != "http" {
        return Err(FetchError::UnsupportedScheme(url.scheme().to_string()));
    }
    let host = url.host();
    let port = url.port_or_default().unwrap_or(80);
    if host.is_empty() {
        return Err(FetchError::InvalidUrl(format!("{url} has no host")));
    }

    let address = (host, port)
        .to_socket_addrs()
        .map_err(|e| FetchError::Io(format!("{url}: {e}")))?
        .next()
        .ok_or_else(|| FetchError::Io(format!("could not resolve {host}")))?;

    let mut stream = TcpStream::connect_timeout(&address, config.timeout)
        .map_err(|e| FetchError::Io(format!("{url}: {e}")))?;
    stream.set_read_timeout(Some(config.timeout)).ok();
    stream.set_write_timeout(Some(config.timeout)).ok();

    let head = request_head(url, method, headers, body, config);
    stream
        .write_all(head.as_bytes())
        .map_err(|e| FetchError::Io(format!("{url}: {e}")))?;
    if let Some(body) = body {
        stream
            .write_all(body)
            .map_err(|e| FetchError::Io(format!("{url}: {e}")))?;
    }

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).map_err(|e| {
        if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut {
            FetchError::Timeout(url.to_string())
        } else {
            FetchError::Io(format!("{url}: {e}"))
        }
    })?;

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
    let host_header = match url.port_or_default() {
        Some(port) if port != 80 => format!("{}:{}", url.host(), port),
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
            // Response field values use HTTP optional whitespace (SP / HTAB),
            // not Rust's broader Unicode whitespace set. Reuse the validated
            // HeaderMap path here so an edge NBSP/EM SPACE remains data instead
            // of being silently erased by the older raw helper.
            headers.append(name, value).ok()?;
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
        let start = line_end + 2;
        let end = (start + size).min(input.len());
        out.extend_from_slice(&input[start..end]);
        // Skip the chunk and its trailing CRLF.
        input = if end + 2 <= input.len() {
            &input[end + 2..]
        } else {
            &[]
        };
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
    fn response_field_values_trim_http_ows_but_preserve_unicode_whitespace() {
        let raw = concat!(
            "HTTP/1.1 200 OK\r\n",
            "X-Ascii:\t value \t\r\n",
            "X-Unicode: \u{00a0}value\u{2003} \r\n",
            "\r\n"
        )
        .as_bytes();
        let response = parse_response(raw).expect("parsed");
        assert_eq!(response.headers.get("x-ascii").as_deref(), Some("value"));
        assert_eq!(
            response.headers.get("x-unicode").as_deref(),
            Some("\u{00a0}value\u{2003}")
        );
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
