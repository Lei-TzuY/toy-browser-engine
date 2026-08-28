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

impl Url {
    /// Serialize this URL's network origin when it has a tuple origin.
    ///
    /// The browser engine currently exposes network tuple origins only for
    /// HTTP(S). Opaque URLs (`data:`, `about:`, `mailto:`, `urn:`), local
    /// `file:` URLs, and the engine-internal `demo:` scheme intentionally
    /// return `None` rather than inventing an origin that could later weaken a
    /// same-origin security check.
    ///
    /// Explicit default ports are normalized away, matching browser origin
    /// serialization: `http://example.test:80/` has origin
    /// `http://example.test` while a non-default port remains visible.
    pub fn origin(&self) -> Option<String> {
        if self.opaque || !matches!(self.scheme.as_str(), "http" | "https") || self.host.is_empty() {
            return None;
        }

        let default_port = match self.scheme.as_str() {
            "http" => 80,
            "https" => 443,
            _ => unreachable!(),
        };

        let mut serialized = format!("{}://{}", self.scheme, self.host);
        if let Some(port) = self.port {
            if port != default_port {
                serialized.push(':');
                serialized.push_str(&port.to_string());
            }
        }
        Some(serialized)
    }

    /// Whether two URLs share the same HTTP(S) tuple origin.
    ///
    /// Query strings, fragments, and paths do not participate in origin
    /// comparison. Explicit and implicit default ports are equivalent. URLs
    /// without a tuple origin deliberately compare as non-same-origin, even
    /// with themselves; this prevents accidental trust of opaque/local URLs.
    pub fn same_origin(&self, other: &Url) -> bool {
        if self.origin().is_none() || other.origin().is_none() {
            return false;
        }

        self.scheme == other.scheme
            && self.host == other.host
            && self.port_or_default() == other.port_or_default()
    }
}
