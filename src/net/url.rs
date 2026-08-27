// ============================================================
//  net/url.rs  —  URL parsing and reference resolution
// ============================================================
//
//  A hierarchical URL (`scheme://host:port/path?query#fragment`) plus the
//  reference-resolution algorithm from RFC 3986 §5, which is what turns
//  `../style.css` in a document into an absolute address the loader can
//  fetch.
//
//  Local files are addressed as `file:///C:/dir/page.html` on Windows and
//  `file:///dir/page.html` elsewhere; `to_file_path` converts back.

use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlError {
    /// No `scheme:` prefix, so the string cannot be an absolute URL.
    MissingScheme(String),
    /// The scheme was present but malformed (empty, or illegal characters).
    InvalidScheme(String),
    /// The port was not a number in range.
    InvalidPort(String),
}

impl fmt::Display for UrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UrlError::MissingScheme(s) => write!(f, "not an absolute URL: {s:?}"),
            UrlError::InvalidScheme(s) => write!(f, "invalid URL scheme: {s:?}"),
            UrlError::InvalidPort(s) => write!(f, "invalid port: {s:?}"),
        }
    }
}

impl std::error::Error for UrlError {}

/// An absolute URL.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Url {
    scheme: String,
    /// Empty for schemes without an authority (`file:///x` has an empty host).
    host: String,
    port: Option<u16>,
    /// Always begins with `/` for hierarchical URLs.
    path: String,
    query: Option<String>,
    fragment: Option<String>,
}

impl Url {
    /// Parse an absolute URL. Relative references belong in [`Url::join`].
    pub fn parse(input: &str) -> Result<Url, UrlError> {
        let trimmed = input.trim();
        let (scheme, rest) =
            split_scheme(trimmed).ok_or_else(|| UrlError::MissingScheme(trimmed.to_string()))?;
        if scheme.is_empty() || !scheme.chars().all(is_scheme_char) {
            return Err(UrlError::InvalidScheme(scheme.to_string()));
        }

        let mut url = Url {
            scheme: scheme.to_ascii_lowercase(),
            host: String::new(),
            port: None,
            path: String::new(),
            query: None,
            fragment: None,
        };

        let after_authority = match rest.strip_prefix("//") {
            Some(rest) => {
                let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
                url.set_authority(&rest[..end])?;
                &rest[end..]
            }
            // Schemes such as `data:` or `about:` keep everything as the path.
            None => rest,
        };

        let (path, query, fragment) = split_path_parts(after_authority);
        url.path = path.to_string();
        url.query = query.map(str::to_string);
        url.fragment = fragment.map(str::to_string);
        url.normalize_path();
        Ok(url)
    }

    /// Resolve a possibly relative reference against this URL (RFC 3986 §5.2).
    ///
    /// Handles absolute URLs, protocol-relative `//host/p`, root-relative
    /// `/p`, relative `p` and `../p`, bare `?query` and `#fragment`, and the
    /// empty reference (which means "this document").
    pub fn join(&self, reference: &str) -> Result<Url, UrlError> {
        let reference = reference.trim();

        // An absolute reference replaces everything.
        if let Ok(absolute) = Url::parse(reference) {
            return Ok(absolute);
        }

        let mut resolved = self.clone();

        // Protocol-relative: keep only the scheme.
        if let Some(rest) = reference.strip_prefix("//") {
            let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
            resolved.host = String::new();
            resolved.port = None;
            resolved.set_authority(&rest[..end])?;
            let (path, query, fragment) = split_path_parts(&rest[end..]);
            resolved.path = path.to_string();
            resolved.query = query.map(str::to_string);
            resolved.fragment = fragment.map(str::to_string);
            resolved.normalize_path();
            return Ok(resolved);
        }

        let (path, query, fragment) = split_path_parts(reference);

        if path.is_empty() {
            // `#frag` and `?query` keep the current path.
            if query.is_some() {
                resolved.query = query.map(str::to_string);
                resolved.fragment = fragment.map(str::to_string);
            } else if fragment.is_some() {
                resolved.fragment = fragment.map(str::to_string);
            } else {
                // Empty reference: same document, fragment dropped.
                resolved.fragment = None;
            }
            return Ok(resolved);
        }

        resolved.path = if path.starts_with('/') {
            path.to_string()
        } else {
            let base_dir = match self.path.rfind('/') {
                Some(i) => &self.path[..=i],
                None => "/",
            };
            format!("{base_dir}{path}")
        };
        resolved.query = query.map(str::to_string);
        resolved.fragment = fragment.map(str::to_string);
        resolved.normalize_path();
        Ok(resolved)
    }

    /// Build a `file://` URL from a filesystem path, absolutizing it against
    /// the current directory when necessary.
    pub fn from_file_path(path: impl AsRef<Path>) -> Url {
        let path = path.as_ref();
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        };

        let mut text = absolute.to_string_lossy().replace('\\', "/");
        if !text.starts_with('/') {
            // Windows drive paths: `C:/dir` becomes `/C:/dir`.
            text.insert(0, '/');
        }

        let mut url = Url {
            scheme: "file".into(),
            host: String::new(),
            port: None,
            path: percent_encode_path(&text),
            query: None,
            fragment: None,
        };
        url.normalize_path();
        url
    }

    /// The filesystem path for a `file:` URL.
    pub fn to_file_path(&self) -> Option<PathBuf> {
        if self.scheme != "file" {
            return None;
        }
        let decoded = percent_decode(&self.path);
        // `/C:/dir/file` is a Windows path; `/dir/file` is a POSIX one.
        let trimmed = decoded.strip_prefix('/').unwrap_or(&decoded);
        let looks_like_drive = trimmed.len() >= 2
            && trimmed.as_bytes()[1] == b':'
            && trimmed.as_bytes()[0].is_ascii_alphabetic();
        if looks_like_drive {
            Some(PathBuf::from(trimmed))
        } else {
            Some(PathBuf::from(decoded))
        }
    }

    pub fn scheme(&self) -> &str {
        &self.scheme
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    /// The port, falling back to the scheme's default where there is one.
    pub fn port_or_default(&self) -> Option<u16> {
        self.port.or(match self.scheme.as_str() {
            "http" => Some(80),
            "https" => Some(443),
            _ => None,
        })
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }

    pub fn fragment(&self) -> Option<&str> {
        self.fragment.as_deref()
    }

    /// Path plus query — what an HTTP request line carries.
    pub fn request_target(&self) -> String {
        match &self.query {
            Some(q) => format!("{}?{}", self.path, q),
            None => self.path.clone(),
        }
    }

    /// The last path segment, useful for display.
    pub fn file_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or("")
    }

    /// Just the scheme, host and path — the identity of the *file* a static
    /// server would serve, with the query and fragment dropped.
    pub fn without_query_and_fragment(&self) -> Url {
        Url {
            query: None,
            fragment: None,
            ..self.clone()
        }
    }

    /// This URL without its fragment — the identity of the *document*.
    pub fn without_fragment(&self) -> Url {
        Url {
            fragment: None,
            ..self.clone()
        }
    }

    /// True when both URLs address the same document (fragments aside).
    pub fn same_document(&self, other: &Url) -> bool {
        self.without_fragment() == other.without_fragment()
    }

    fn set_authority(&mut self, authority: &str) -> Result<(), UrlError> {
        // Strip any `user:pass@` prefix — credentials are not used here.
        let host_port = authority.rsplit('@').next().unwrap_or(authority);

        // An IPv6 literal is full of colons, so the port separator is only the
        // colon that follows the closing bracket.
        let separator = if host_port.starts_with('[') {
            host_port
                .rfind(']')
                .and_then(|end| host_port[end..].find(':').map(|offset| end + offset))
        } else {
            host_port.find(':')
        };

        match separator {
            Some(index) => {
                self.host = host_port[..index].to_ascii_lowercase();
                let port = &host_port[index + 1..];
                self.port = if port.is_empty() {
                    None
                } else {
                    Some(
                        port.parse()
                            .map_err(|_| UrlError::InvalidPort(port.to_string()))?,
                    )
                };
            }
            None => self.host = host_port.to_ascii_lowercase(),
        }
        Ok(())
    }

    /// Apply RFC 3986 `remove_dot_segments` and ensure a leading `/`.
    fn normalize_path(&mut self) {
        if self.path.is_empty() {
            self.path = "/".into();
            return;
        }
        if !self.path.starts_with('/') && !self.host.is_empty() {
            self.path.insert(0, '/');
        }

        let had_trailing_slash =
            self.path.ends_with('/') || self.path.ends_with("/.") || self.path.ends_with("/..");

        let mut output: Vec<&str> = Vec::new();
        for segment in self.path.split('/') {
            match segment {
                "" | "." => {}
                ".." => {
                    output.pop();
                }
                other => output.push(other),
            }
        }

        let mut path = String::new();
        for segment in &output {
            path.push('/');
            path.push_str(segment);
        }
        if path.is_empty() || (had_trailing_slash && !path.ends_with('/')) {
            path.push('/');
        }
        self.path = path;
    }
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:", self.scheme)?;
        if !self.host.is_empty() || self.scheme == "file" || self.scheme == "demo" {
            write!(f, "//{}", self.host)?;
            if let Some(port) = self.port {
                write!(f, ":{port}")?;
            }
        }
        write!(f, "{}", self.path)?;
        if let Some(query) = &self.query {
            write!(f, "?{query}")?;
        }
        if let Some(fragment) = &self.fragment {
            write!(f, "#{fragment}")?;
        }
        Ok(())
    }
}

// ── Parsing helpers ───────────────────────────────────────────────────────────

fn is_scheme_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.'
}

/// Split `scheme:rest`, requiring the scheme to start with a letter.
///
/// RFC 3986 permits a one-character scheme. Keep the common backslash Windows
/// drive spelling (`C:\\dir`) out of the URI path without rejecting legitimate
/// one-letter URI schemes such as `x:/resource`.
fn split_scheme(input: &str) -> Option<(&str, &str)> {
    let colon = input.find(':')?;
    let scheme = &input[..colon];
    if scheme.is_empty() || !scheme.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return None;
    }
    let rest = &input[colon + 1..];
    if scheme.len() == 1 && rest.starts_with('\\') {
        return None;
    }
    Some((scheme, rest))
}

/// Split a reference into `(path, query, fragment)`.
fn split_path_parts(input: &str) -> (&str, Option<&str>, Option<&str>) {
    let (before_fragment, fragment) = match input.find('#') {
        Some(i) => (&input[..i], Some(&input[i + 1..])),
        None => (input, None),
    };
    let (path, query) = match before_fragment.find('?') {
        Some(i) => (&before_fragment[..i], Some(&before_fragment[i + 1..])),
        None => (before_fragment, None),
    };
    (path, query, fragment)
}

/// Percent-decode `%XX` escapes.
pub fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Percent-encode the characters that are not allowed raw in a path.
fn percent_encode_path(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            ' ' => out.push_str("%20"),
            '"' => out.push_str("%22"),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            '`' => out.push_str("%60"),
            '#' => out.push_str("%23"),
            '?' => out.push_str("%3F"),
            other => out.push(other),
        }
    }
    out
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).expect("valid url")
    }

    #[test]
    fn parses_the_parts_of_an_http_url() {
        let u = url("http://example.com:8080/a/b.html?x=1&y=2#top");
        assert_eq!(u.scheme(), "http");
        assert_eq!(u.host(), "example.com");
        assert_eq!(u.port_or_default(), Some(8080));
        assert_eq!(u.path(), "/a/b.html");
        assert_eq!(u.query(), Some("x=1&y=2"));
        assert_eq!(u.fragment(), Some("top"));
        assert_eq!(u.request_target(), "/a/b.html?x=1&y=2");
    }

    #[test]
    fn single_letter_schemes_are_valid_absolute_urls() {
        let u = url("X:/resource?q=1#frag");
        assert_eq!(u.scheme(), "x");
        assert_eq!(u.path(), "/resource");
        assert_eq!(u.query(), Some("q=1"));
        assert_eq!(u.fragment(), Some("frag"));
        assert_eq!(u.to_string(), "x:/resource?q=1#frag");
    }

    #[test]
    fn default_ports_come_from_the_scheme() {
        assert_eq!(url("http://example.com/").port_or_default(), Some(80));
        assert_eq!(url("https://example.com/").port_or_default(), Some(443));
        assert_eq!(url("file:///tmp/x").port_or_default(), None);
    }

    #[test]
    fn round_trips_through_display() {
        for text in [
            "http://example.com/a/b?c=d#e",
            "https://example.com/",
            "file:///tmp/page.html",
        ] {
            assert_eq!(url(text).to_string(), text);
        }
    }

    #[test]
    fn rejects_relative_references_as_absolute_urls() {
        assert!(matches!(
            Url::parse("style.css"),
            Err(UrlError::MissingScheme(_))
        ));
        assert!(matches!(
            Url::parse("/root/style.css"),
            Err(UrlError::MissingScheme(_))
        ));
        // A backslash Windows path must not be read as a `c:` scheme.
        assert!(Url::parse(r"C:\dir\page.html").is_err());
    }

    #[test]
    fn joins_relative_paths() {
        let base = url("http://example.com/docs/guide/index.html");
        assert_eq!(
            base.join("style.css").unwrap().to_string(),
            "http://example.com/docs/guide/style.css"
        );
        assert_eq!(
            base.join("./style.css").unwrap().to_string(),
            "http://example.com/docs/guide/style.css"
        );
        assert_eq!(
            base.join("../style.css").unwrap().to_string(),
            "http://example.com/docs/style.css"
        );
        assert_eq!(
            base.join("../../style.css").unwrap().to_string(),
            "http://example.com/style.css"
        );
        assert_eq!(
            base.join("/style.css").unwrap().to_string(),
            "http://example.com/style.css"
        );
        assert_eq!(
            base.join("sub/page.html").unwrap().to_string(),
            "http://example.com/docs/guide/sub/page.html"
        );
    }

    #[test]
    fn joins_absolute_and_protocol_relative_references() {
        let base = url("https://example.com/a/b.html");
        assert_eq!(
            base.join("http://other.test/x").unwrap().to_string(),
            "http://other.test/x"
        );
        assert_eq!(
            base.join("//cdn.test/lib.js").unwrap().to_string(),
            "https://cdn.test/lib.js"
        );
    }

    #[test]
    fn joins_query_and_fragment_only_references() {
        let base = url("http://example.com/a/b.html?old=1#old");
        assert_eq!(
            base.join("#section").unwrap().to_string(),
            "http://example.com/a/b.html?old=1#section"
        );
        assert_eq!(
            base.join("?new=2").unwrap().to_string(),
            "http://example.com/a/b.html?new=2"
        );
        assert_eq!(
            base.join("").unwrap().to_string(),
            "http://example.com/a/b.html?old=1"
        );
    }

    #[test]
    fn dot_segments_cannot_escape_the_root() {
        let base = url("http://example.com/a.html");
        assert_eq!(
            base.join("../../../x.css").unwrap().to_string(),
            "http://example.com/x.css"
        );
    }

    #[test]
    fn directory_bases_keep_their_trailing_slash() {
        let base = url("http://example.com/docs/");
        assert_eq!(
            base.join("a.html").unwrap().to_string(),
            "http://example.com/docs/a.html"
        );
        assert_eq!(
            base.join("../a.html").unwrap().to_string(),
            "http://example.com/a.html"
        );
    }

    #[test]
    fn file_urls_round_trip_through_paths() {
        let original = std::env::current_dir()
            .unwrap()
            .join("sub")
            .join("page.html");
        let u = Url::from_file_path(&original);
        assert_eq!(u.scheme(), "file");
        assert_eq!(u.to_file_path().unwrap(), original);
    }

    #[test]
    fn file_urls_resolve_relative_references() {
        let base = Url::from_file_path("/site/docs/index.html");
        let joined = base.join("../assets/logo.png").unwrap();
        assert_eq!(joined.scheme(), "file");
        assert!(
            joined.path().ends_with("/site/assets/logo.png"),
            "got {joined}"
        );
    }

    #[test]
    fn spaces_in_paths_are_encoded_and_decoded() {
        // Built from a real absolute path so the test holds on every platform.
        let path = std::env::temp_dir().join("my dir").join("a b.html");
        let u = Url::from_file_path(&path);
        assert!(u.to_string().contains("%20"), "got {u}");
        assert_eq!(u.to_file_path().unwrap(), path);
    }

    #[test]
    fn dropping_the_query_gives_the_file_identity() {
        let u = url("http://example.com/search?q=1#top");
        assert_eq!(
            u.without_query_and_fragment().to_string(),
            "http://example.com/search"
        );
    }

    #[test]
    fn same_document_ignores_the_fragment() {
        let a = url("http://example.com/x.html#one");
        let b = url("http://example.com/x.html#two");
        let c = url("http://example.com/y.html");
        assert!(a.same_document(&b));
        assert!(!a.same_document(&c));
    }

    #[test]
    fn credentials_and_ipv6_hosts_parse() {
        assert_eq!(url("http://user:pw@example.com/x").host(), "example.com");
        assert_eq!(url("http://[::1]:9000/x").host(), "[::1]");
        assert_eq!(url("http://[::1]:9000/x").port_or_default(), Some(9000));
    }

    #[test]
    fn hosts_and_schemes_are_lowercased() {
        let u = url("HTTP://Example.COM/Path");
        assert_eq!(u.scheme(), "http");
        assert_eq!(u.host(), "example.com");
        // Paths stay case-sensitive.
        assert_eq!(u.path(), "/Path");
    }
}
