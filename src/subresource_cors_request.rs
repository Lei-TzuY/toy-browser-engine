//! Request preparation for HTML CORS-enabled subresources.
//!
//! A present `crossorigin` attribute switches an element fetch into CORS mode.
//! CORS-mode requests carry a browser-owned `Origin` header derived from the
//! owning document; callers must not be able to preserve a stale or forged
//! serialized origin across redirects or reuse.

use crate::cookie_same_site::SameSiteRequestContext;
use crate::cors_settings::parse_cors_settings_attribute;
use crate::document_referrer::DocumentReferrerContext;
use crate::navigation_network::NavigationNetwork;
use crate::net::{FetchError, FetchRequest, FetchResponse, Origin, Url};
use crate::subresource_cors::validate_subresource_cors_response;

/// Prepare one element-initiated request for HTML CORS mode.
///
/// Missing `crossorigin` keeps the existing no-CORS request untouched. Any
/// present value enables CORS (invalid/empty values resolve to Anonymous), so
/// the browser replaces any caller-supplied `Origin` header with the owning
/// document's serialized origin. A CORS request without an owning origin fails
/// closed instead of emitting an invented value.
pub fn prepare_cors_subresource_request(
    source: Option<&Url>,
    crossorigin: Option<&str>,
    request: &FetchRequest,
) -> Result<FetchRequest, FetchError> {
    if parse_cors_settings_attribute(crossorigin).is_none() {
        return Ok(request.clone());
    }

    let Some(source) = source else {
        return Err(FetchError::Blocked(
            "CORS: CORS-enabled subresource has no request origin".to_string(),
        ));
    };

    let mut prepared = request.clone();
    prepared.headers.delete("origin");
    prepared
        .headers
        .insert_raw("origin", &Origin::of(source).header_value());
    Ok(prepared)
}

/// Additional request-side CORS entry point for committed documents.
///
/// This composes the browser-owned `Origin` header with the existing referrer,
/// redirect, Cookie/HSTS, and final CORS-response validation layers. It is kept
/// as an additive API so existing callers remain source-compatible while the
/// engine grows the Fetch CORS state machine incrementally.
impl DocumentReferrerContext {
    pub fn fetch_cors_subresource(
        &self,
        network: &NavigationNetwork,
        request: &FetchRequest,
        context: SameSiteRequestContext,
        referrerpolicy: Option<&str>,
        crossorigin: Option<&str>,
    ) -> Result<FetchResponse, FetchError> {
        let prepared = prepare_cors_subresource_request(self.source(), crossorigin, request)?;
        let response = self.fetch_subresource(network, &prepared, context, referrerpolicy)?;
        validate_subresource_cors_response(self.source(), crossorigin, &response)?;
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::Url;

    fn url(input: &str) -> Url {
        Url::parse(input).unwrap()
    }

    #[test]
    fn missing_crossorigin_preserves_existing_request() {
        let mut request = FetchRequest::get(url("https://cdn.test/image.png"));
        request.headers.insert_raw("origin", "https://caller.test");

        let prepared = prepare_cors_subresource_request(
            Some(&url("https://page.test/index.html")),
            None,
            &request,
        )
        .unwrap();

        assert_eq!(prepared.headers.get("origin").as_deref(), Some("https://caller.test"));
    }

    #[test]
    fn cors_request_replaces_forged_origin_with_document_origin() {
        let mut request = FetchRequest::get(url("https://cdn.test/image.png"));
        request.headers.insert_raw("origin", "https://evil.test");

        let prepared = prepare_cors_subresource_request(
            Some(&url("https://page.test/private/index.html")),
            Some("anonymous"),
            &request,
        )
        .unwrap();

        assert_eq!(prepared.headers.get("origin").as_deref(), Some("https://page.test"));
    }

    #[test]
    fn use_credentials_also_carries_browser_owned_origin() {
        let request = FetchRequest::get(url("https://cdn.test/script.js"));
        let prepared = prepare_cors_subresource_request(
            Some(&url("https://page.test:8443/index.html")),
            Some("use-credentials"),
            &request,
        )
        .unwrap();

        assert_eq!(
            prepared.headers.get("origin").as_deref(),
            Some("https://page.test:8443")
        );
    }

    #[test]
    fn cors_request_without_document_origin_fails_closed() {
        let request = FetchRequest::get(url("https://cdn.test/image.png"));
        assert!(prepare_cors_subresource_request(None, Some("anonymous"), &request).is_err());
    }
}
