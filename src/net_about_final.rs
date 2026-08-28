// ============================================================
// net_about_final.rs — public network facade with about:blank support
// ============================================================

pub use crate::net_prev2::*;

/// Loader for browser-internal `about:` documents.
///
/// `about:blank` is a navigation/document resource rather than a network
/// endpoint. Keeping it behind `ResourceLoader::load` means Browser and
/// Document need no special-case: open/navigate/reload/history all continue to
/// use the same resource pipeline as ordinary pages.
#[derive(Debug, Default, Clone, Copy)]
pub struct AboutLoader;

impl AboutLoader {
    pub const fn new() -> Self {
        Self
    }
}

impl ResourceLoader for AboutLoader {
    fn load(&self, url: &Url) -> Result<Resource, LoadError> {
        if url.scheme() != "about" {
            return Err(LoadError::UnsupportedScheme(url.scheme().to_string()));
        }
        if !url.path().eq_ignore_ascii_case("blank") {
            return Err(LoadError::NotFound(url.to_string()));
        }

        // Use a real minimal HTML document rather than an empty byte string so
        // the existing HTML parser reliably creates the normal document tree.
        // Query and fragment components affect the visible URL/history entry,
        // not the bytes of this browser-internal document.
        const BLANK_HTML: &[u8] = b"<!doctype html><html><head></head><body></body></html>";
        Ok(Resource::new(
            url.clone(),
            Some("text/html".to_string()),
            BLANK_HTML.to_vec(),
        ))
    }
}

/// Scheme-routing loader used by the CLI and embedders.
///
/// The prior facade retains file/http/memory/data behaviour. This layer only
/// intercepts browser-internal `about:` document loads.
pub struct DefaultLoader {
    inner: crate::net_prev2::DefaultLoader,
}

impl DefaultLoader {
    pub fn new() -> Self {
        Self {
            inner: crate::net_prev2::DefaultLoader::new(),
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
        if url.scheme() == "about" {
            AboutLoader.load(url)
        } else {
            self.inner.load(url)
        }
    }

    fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse, FetchError> {
        // Browser-internal pages are navigation resources, not Fetch endpoints.
        // This also avoids accidentally giving `about:` the local-file origin
        // semantics of a hostless URL in the script networking layer.
        if request.url.scheme() == "about" {
            Err(FetchError::UnsupportedScheme("about".to_string()))
        } else {
            self.inner.fetch(request)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn about(input: &str) -> Url {
        Url::parse(input).expect("valid about URL")
    }

    #[test]
    fn loads_about_blank_as_html() {
        let resource = AboutLoader.load(&about("about:blank")).unwrap();
        assert_eq!(resource.effective_mime(), "text/html");
        assert!(resource.text().contains("<body>"));
        assert_eq!(resource.url.to_string(), "about:blank");
    }

    #[test]
    fn query_and_fragment_do_not_change_blank_document_bytes() {
        let plain = AboutLoader.load(&about("about:blank")).unwrap();
        let decorated = AboutLoader
            .load(&about("about:blank?debug=1#section"))
            .unwrap();
        assert_eq!(decorated.bytes, plain.bytes);
        assert_eq!(decorated.url.to_string(), "about:blank?debug=1#section");
    }

    #[test]
    fn unknown_about_page_is_not_found() {
        assert!(matches!(
            AboutLoader.load(&about("about:config")),
            Err(LoadError::NotFound(_))
        ));
    }

    #[test]
    fn default_loader_does_not_expose_about_through_fetch() {
        let request = FetchRequest::get(about("about:blank"));
        assert!(matches!(
            DefaultLoader::new().fetch(&request),
            Err(FetchError::UnsupportedScheme(scheme)) if scheme == "about"
        ));
    }
}
