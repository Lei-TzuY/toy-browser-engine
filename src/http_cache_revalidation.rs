//! HTTP cache revalidation primitives.
//!
//! RFC 9111 revalidation uses validators carried by the stored response to
//! construct conditional requests. Entity tags are sent in `If-None-Match` and
//! Last-Modified values in `If-Modified-Since`; when both are available a cache
//! can send both validators so the origin can select the strongest applicable
//! condition.

use crate::net::fetch::HeaderMap;

/// Validators retained from a stored response for later cache revalidation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HttpCacheValidators {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

impl HttpCacheValidators {
    /// Return whether the stored response has any usable revalidation validator.
    pub fn is_empty(&self) -> bool {
        self.etag.is_none() && self.last_modified.is_none()
    }
}

/// Extract validators from a stored HTTP response.
///
/// Values are preserved verbatim apart from surrounding optional whitespace.
/// Empty field values are ignored. Weak entity tags remain valid validators and
/// are intentionally preserved rather than upgraded to strong tags.
pub fn response_cache_validators(headers: &HeaderMap) -> HttpCacheValidators {
    HttpCacheValidators {
        etag: non_empty_trimmed(headers.get("etag")),
        last_modified: non_empty_trimmed(headers.get("last-modified")),
    }
}

/// Build the conditional request fields needed to revalidate a cached response.
///
/// `If-None-Match` is emitted for an ETag and `If-Modified-Since` for a
/// Last-Modified validator. If both validators exist both request fields are
/// emitted. No synthetic condition is invented when a stored response has no
/// validator.
pub fn cache_revalidation_headers(validators: &HttpCacheValidators) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(etag) = validators.etag.as_deref() {
        headers.insert_raw("if-none-match", etag);
    }
    if let Some(last_modified) = validators.last_modified.as_deref() {
        headers.insert_raw("if-modified-since", last_modified);
    }
    headers
}

/// A 304 response means the selected stored representation was not modified.
pub fn response_confirms_not_modified(status: u16) -> bool {
    status == 304
}

fn non_empty_trimmed(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim();
        if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_both_response_validators() {
        let mut headers = HeaderMap::new();
        headers.insert_raw("etag", "  W/\"v7\"  ");
        headers.insert_raw("last-modified", " Sun, 30 Aug 2026 10:00:00 GMT ");

        let validators = response_cache_validators(&headers);
        assert_eq!(validators.etag.as_deref(), Some("W/\"v7\""));
        assert_eq!(
            validators.last_modified.as_deref(),
            Some("Sun, 30 Aug 2026 10:00:00 GMT")
        );
    }

    #[test]
    fn weak_etags_are_preserved() {
        let mut headers = HeaderMap::new();
        headers.insert_raw("etag", "W/\"weak\"");
        let validators = response_cache_validators(&headers);
        assert_eq!(validators.etag.as_deref(), Some("W/\"weak\""));
    }

    #[test]
    fn empty_validator_fields_are_ignored() {
        let mut headers = HeaderMap::new();
        headers.insert_raw("etag", "   ");
        headers.insert_raw("last-modified", "");
        assert!(response_cache_validators(&headers).is_empty());
    }

    #[test]
    fn revalidation_request_emits_both_conditions() {
        let validators = HttpCacheValidators {
            etag: Some("\"abc\"".into()),
            last_modified: Some("Sun, 30 Aug 2026 10:00:00 GMT".into()),
        };
        let headers = cache_revalidation_headers(&validators);
        assert_eq!(headers.get("if-none-match").as_deref(), Some("\"abc\""));
        assert_eq!(
            headers.get("if-modified-since").as_deref(),
            Some("Sun, 30 Aug 2026 10:00:00 GMT")
        );
    }

    #[test]
    fn no_validator_produces_no_conditions() {
        let headers = cache_revalidation_headers(&HttpCacheValidators::default());
        assert!(headers.get("if-none-match").is_none());
        assert!(headers.get("if-modified-since").is_none());
    }

    #[test]
    fn only_304_confirms_not_modified() {
        assert!(response_confirms_not_modified(304));
        for status in [200, 204, 301, 302, 412, 500] {
            assert!(!response_confirms_not_modified(status));
        }
    }
}
