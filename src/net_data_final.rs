// ============================================================
// net_data_final.rs — public network facade with data: URL support
// ============================================================

pub use crate::net_prev::*;

/// Decoded payload of a `data:` URL before it is wrapped as a [`Resource`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedDataUrl {
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// Loader for RFC 2397 `data:` resources.
///
/// Both percent-encoded payloads and the common `;base64` form are supported.
/// The URL fragment is never part of the resource body. The existing URL type
/// currently normalizes opaque paths with one synthetic leading slash, so the
/// decoder removes exactly that routing artifact before interpreting the data
/// URL grammar.
#[derive(Debug, Default, Clone, Copy)]
pub struct DataLoader;

impl DataLoader {
    pub const fn new() -> Self {
        Self
    }
}

impl ResourceLoader for DataLoader {
    fn load(&self, url: &Url) -> Result<Resource, LoadError> {
        let decoded = decode_data_url(url)?;
        Ok(Resource::new(
            url.clone(),
            Some(decoded.mime),
            decoded.bytes,
        ))
    }
}

/// Decode a `data:` URL into MIME metadata and bytes.
///
/// With no media type, RFC 2397 defaults to `text/plain`. Parameters such as
/// `charset=UTF-8` are accepted but the engine's `Resource` model stores only
/// the lower-cased essence MIME type, consistently with HTTP resources.
pub fn decode_data_url(url: &Url) -> Result<DecodedDataUrl, LoadError> {
    if url.scheme() != "data" {
        return Err(LoadError::UnsupportedScheme(url.scheme().to_string()));
    }

    let mut opaque = url.path();
    if let Some(stripped) = opaque.strip_prefix('/') {
        opaque = stripped;
    }

    // `?` is legal data payload content. The generic URL parser stores it as
    // a query component, so restore it before parsing the RFC 2397 comma split.
    let serialized;
    if let Some(query) = url.query() {
        serialized = format!("{opaque}?{query}");
        opaque = &serialized;
    }

    let (metadata, payload) = opaque
        .split_once(',')
        .ok_or_else(|| invalid_data_url(url, "missing comma separator"))?;

    let mut fields = metadata.split(';');
    let first = fields.next().unwrap_or("").trim();
    let mime = if first.is_empty() {
        "text/plain".to_string()
    } else {
        if !first.contains('/') || first.chars().any(|c| c.is_ascii_whitespace()) {
            return Err(invalid_data_url(url, "invalid media type"));
        }
        first.to_ascii_lowercase()
    };

    let mut base64 = false;
    for parameter in fields {
        let parameter = parameter.trim();
        if parameter.eq_ignore_ascii_case("base64") {
            if base64 {
                return Err(invalid_data_url(url, "duplicate base64 marker"));
            }
            base64 = true;
        }
        // Other parameters (for example charset=UTF-8) are intentionally
        // accepted even though Resource currently stores only the MIME essence.
    }

    let encoded = percent_decode_bytes(payload)
        .map_err(|reason| invalid_data_url(url, reason))?;
    let bytes = if base64 {
        decode_forgiving_base64(&encoded).map_err(|reason| invalid_data_url(url, reason))?
    } else {
        encoded
    };

    Ok(DecodedDataUrl { mime, bytes })
}

fn invalid_data_url(url: &Url, reason: impl Into<String>) -> LoadError {
    LoadError::InvalidUrl(format!("{} ({})", url, reason.into()))
}

fn percent_decode_bytes(input: &str) -> Result<Vec<u8>, &'static str> {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'%' {
            output.push(bytes[i]);
            i += 1;
            continue;
        }
        if i + 2 >= bytes.len() {
            return Err("truncated percent escape");
        }
        let hi = hex_value(bytes[i + 1]).ok_or("invalid percent escape")?;
        let lo = hex_value(bytes[i + 2]).ok_or("invalid percent escape")?;
        output.push((hi << 4) | lo);
        i += 3;
    }
    Ok(output)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// WHATWG-style forgiving base64 for data URLs: ASCII whitespace is ignored
/// and final padding may be omitted, while the alphabet and padding placement
/// remain strict enough to reject malformed resources deterministically.
fn decode_forgiving_base64(input: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut clean: Vec<u8> = input
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();

    let mut padding = 0usize;
    while clean.last() == Some(&b'=') {
        clean.pop();
        padding += 1;
    }
    if padding > 2 || clean.contains(&b'=') {
        return Err("invalid base64 padding");
    }

    let remainder = clean.len() % 4;
    if remainder == 1 {
        return Err("invalid base64 length");
    }
    if (padding == 1 && remainder != 3) || (padding == 2 && remainder != 2) {
        return Err("invalid base64 padding");
    }

    let mut output = Vec::with_capacity((clean.len() * 3) / 4 + 2);
    for chunk in clean.chunks(4) {
        let mut value = 0u32;
        for &byte in chunk {
            value = (value << 6) | u32::from(base64_value(byte).ok_or("invalid base64 digit")?);
        }
        let missing = 4 - chunk.len();
        value <<= missing * 6;
        output.push(((value >> 16) & 0xff) as u8);
        if chunk.len() >= 3 {
            output.push(((value >> 8) & 0xff) as u8);
        }
        if chunk.len() == 4 {
            output.push((value & 0xff) as u8);
        }
    }
    Ok(output)
}

fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Scheme-routing loader used by the CLI and embedders.
///
/// All existing file/http/in-memory behaviour remains delegated to the prior
/// facade; only `data:` is intercepted here.
pub struct DefaultLoader {
    inner: crate::net_prev::DefaultLoader,
}

impl DefaultLoader {
    pub fn new() -> Self {
        Self {
            inner: crate::net_prev::DefaultLoader::new(),
        }
    }

    pub fn with_memory(self, memory: MemoryLoader) -> Self {
        Self {
            inner: self.inner.with_memory(memory),
        }
    }

    pub fn with_http_config(self, config: HttpConfig) -> Self {
        Self {
            inner: self.inner.with_http_config(config),
        }
    }
}

impl Default for DefaultLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceLoader for DefaultLoader {
    fn load(&self, url: &Url) -> Result<Resource, LoadError> {
        if url.scheme() == "data" {
            DataLoader.load(url)
        } else {
            self.inner.load(url)
        }
    }

    fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        if request.url.scheme() == "data" {
            DataLoader.fetch(request)
        } else {
            self.inner.fetch(request)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(input: &str) -> Url {
        Url::parse(input).expect("valid absolute data URL")
    }

    #[test]
    fn decodes_percent_encoded_text_with_default_mime() {
        let decoded = decode_data_url(&data("data:,Hello%2C%20World%21")).unwrap();
        assert_eq!(decoded.mime, "text/plain");
        assert_eq!(decoded.bytes, b"Hello, World!");
    }

    #[test]
    fn decodes_base64_and_ignores_ascii_whitespace() {
        let decoded = decode_data_url(&data("data:text/plain;base64,SGVs%20bG8h")).unwrap();
        assert_eq!(decoded.mime, "text/plain");
        assert_eq!(decoded.bytes, b"Hello!");
    }

    #[test]
    fn preserves_question_mark_payload_content() {
        let decoded = decode_data_url(&data("data:text/plain,what?yes")).unwrap();
        assert_eq!(decoded.bytes, b"what?yes");
    }

    #[test]
    fn default_loader_routes_data_without_memory_registration() {
        let resource = DefaultLoader::new()
            .load(&data("data:application/json,%7B%22ok%22%3Atrue%7D"))
            .unwrap();
        assert_eq!(resource.effective_mime(), "application/json");
        assert_eq!(resource.bytes, br#"{"ok":true}"#);
    }

    #[test]
    fn rejects_bad_percent_and_base64_payloads() {
        assert!(matches!(
            DataLoader.load(&data("data:,bad%2")),
            Err(LoadError::InvalidUrl(_))
        ));
        assert!(matches!(
            DataLoader.load(&data("data:;base64,abcde")),
            Err(LoadError::InvalidUrl(_))
        ));
    }
}
