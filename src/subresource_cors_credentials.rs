//! Request-side credential handling for HTML CORS-enabled subresources.
//!
//! PR #157 established the response permission gate. This layer supplies the
//! other half of the contract without changing existing response-only APIs:
//! the browser owns `Origin`, anonymous CORS uses same-origin credentials, and
//! `use-credentials` permits cross-origin credentials subject to cookie policy.

use crate::cookie_same_site::SameSiteRequestContext;
use crate::cors_settings::{
    parse_cors_settings_attribute, CorsCredentialsMode, CorsSettingsAttribute,
};
use crate::document_referrer::DocumentReferrerContext;
use crate::navigation_network::{NavigationNetwork, NetworkCredentialsMode};
use crate::net::{FetchError, FetchRequest, FetchResponse, Origin};
use crate::subresource_cors::validate_subresource_cors_response;

impl DocumentReferrerContext {
    /// Fetch a CORS-aware element subresource with request credential semantics.
    ///
    /// This is the credential-aware companion to
    /// [`DocumentReferrerContext::fetch_subresource_with_cors`]. A missing
    /// `crossorigin` attribute preserves the established no-CORS path. When the
    /// attribute is present:
    ///
    /// - the browser replaces any authored `Origin` header with the committed
    ///   document origin;
    /// - `anonymous` (including empty/invalid values) uses Fetch's
    ///   `same-origin` credential mode;
    /// - `use-credentials` uses `include`, while the cookie jar still enforces
    ///   Domain/Path/Secure/SameSite policy;
    /// - cookie eligibility is recomputed for every redirect hop after HSTS;
    /// - `Set-Cookie` is ignored on hops where credentials are not eligible;
    /// - the final response must still pass the CORS response gate.
    pub fn fetch_subresource_with_cors_credentials(
        &self,
        network: &NavigationNetwork,
        request: &FetchRequest,
        context: SameSiteRequestContext,
        referrerpolicy: Option<&str>,
        crossorigin: Option<&str>,
    ) -> Result<FetchResponse, FetchError> {
        let Some(setting) = parse_cors_settings_attribute(crossorigin) else {
            return self.fetch_subresource_with_cors(
                network,
                request,
                context,
                referrerpolicy,
                crossorigin,
            );
        };

        let source = self.source().ok_or_else(|| {
            FetchError::Blocked("CORS: CORS-enabled subresource has no request origin".into())
        })?;

        let mut cors_request = request.clone();
        cors_request.headers.delete("origin");
        cors_request
            .headers
            .insert_raw("origin", &Origin::of(source).header_value());

        let credentials_mode = match setting.credentials_mode() {
            CorsCredentialsMode::SameOrigin => NetworkCredentialsMode::SameOrigin,
            CorsCredentialsMode::Include => NetworkCredentialsMode::Include,
        };

        let response = network.fetch_with_referrer_and_credentials(
            &cors_request,
            context,
            Some(self.subresource_redirect_state(referrerpolicy)),
            credentials_mode,
            Some(source),
        )?;

        validate_subresource_cors_response(Some(source), crossorigin, &response)?;
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_map_to_expected_network_credential_modes() {
        assert_eq!(
            CorsSettingsAttribute::Anonymous.credentials_mode(),
            CorsCredentialsMode::SameOrigin
        );
        assert_eq!(
            CorsSettingsAttribute::UseCredentials.credentials_mode(),
            CorsCredentialsMode::Include
        );
    }
}
