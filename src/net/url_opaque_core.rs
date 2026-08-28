// ============================================================
//  net/url.rs  —  URL parsing and reference resolution
// ============================================================
//
//  Hierarchical URLs (`scheme://host:port/path?query#fragment`) use RFC 3986
//  reference resolution and dot-segment removal. Opaque URLs such as `data:`
//  and `about:` preserve their scheme-specific path verbatim and are not
//  treated as directory bases for relative references.
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
    /// Begins with `/` for hierarchical URLs; opaque URLs preserve their raw
    /// scheme-specific path without an artificial leading slash.
    path: String,
    query: Option<String>,
    fragment: Option<String>,
    /// True for non-hierarchical forms such as `data:text/plain,hello` and
    /// `about:blank`. These URLs cannot resolve ordinary relative paths.
    opaque: bool,
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

        let scheme = scheme.to_ascii_lowercase();
        // Some schemes have well-defined path semantics independent of the
        // spelling after `:`. Preserve those invariants explicitly rather than
        // inferring everything from a leading slash. For unknown schemes,
        // `foo:bar` remains opaque while `foo:/bar` and `foo://host/bar` are
        // treated as hierarchical, which is the least surprising RFC-3986
        // fallback for an embedder-defined scheme.
        let opaque = if scheme_has_opaque_path(&scheme) {
            true
        } else if scheme_is_hierarchical(&scheme) {
            false
        } else {
            !rest.starts_with("//") && !rest.starts_with('/')
        };
        let mut url = Url {
            scheme,
            host: String::new(),
            port: None,
            path: String::new(),
            query: None,
            fragment: None,
            opaque,
        };

        // `//` only introduces an authority for hierarchical URLs. For an
        // intrinsically opaque scheme it is ordinary scheme-specific data.
        let after_authority = if !url.opaque {
            match rest.strip_prefix("//") {
                Some(rest) => {
                    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
                    url.set_authority(&rest[..end])?;
                    &rest[end..]
                }
                None => rest,
            }
        } else {
            rest
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
    /// Hierarchical bases handle absolute URLs, protocol-relative `//host/p`,
    /// root-relative `/p`, relative `p` and `../p`, bare `?query` and
    /// `#fragment`, and the empty reference. Opaque bases only accept an
    /// absolute replacement or an empty/query/fragment-only reference.
    pub fn join(&self, reference: &str) -> Result<Url, UrlError> {
        let reference = reference.trim();

        // An absolute reference replaces everything, even for an opaque base.
        if let Ok(absolute) = Url::parse(reference) {
            return Ok(absolute);
        }

        let (path, query, fragment) = split_path_parts(reference);
        if self.opaque {
            if !path.is_empty() {
                return Err(UrlError::MissingScheme(format!(
                    "{reference} (base URL is opaque: {self})"
                )));
            }
            let mut resolved = self.clone();
            if query.is_some() {
                resolved.query = query.map(str::to_string);
                resolved.fragment = fragment.map(str::to_string);
            } else if fragment.is_some() {
                resolved.fragment = fragment.map(str::to_string);
            } else {
                resolved.fragment = None;
            }
            return Ok(resolved);
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
            opaque: false,
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

    /// True when the scheme-specific part is opaque rather than hierarchical.
    pub fn is_opaque(&self) -> bool {
        self.opaque
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
        self.opaque = false;
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

    /// Apply RFC 3986 `remove_dot_segments` and ensure a leading `/` to a
    /// hierarchical path. Opaque scheme-specific paths are preserved verbatim.
    fn normalize_path(&mut self) {
        if self.opaque {
            return;
        }
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

/// Schemes whose post-colon text is intrinsically opaque in the subset this
/// engine models. A leading slash or `//` inside these schemes is data, not a
/// signal to reinterpret the URL as hierarchical.
fn scheme_has_opaque_path(scheme: &str) -> bool {
    matches!(scheme, "data" | "about" | "mailto" | "urn")
}

/// Schemes already routed by the engine as hierarchical resource locations.
/// Preserve their historical path behavior even for uncommon rootless
/// spellings such as `http:foo` or `file:relative.txt`.
fn scheme_is_hierarchical(scheme: &str) -> bool {
    matches!(scheme, "http" | "https" | "file" | "demo")
}
