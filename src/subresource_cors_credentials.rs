//! Request-side credential handling for HTML CORS-enabled subresources.
//!
//! PR #157 established the response permission gate. This layer supplies the
//! other half of the contract without changing existing response-only APIs:
//! the browser owns `Origin`, anonymous CORS uses same-origin credentials,
//! `use-credentials` permits cross-origin credentials subject to cookie policy,
//! and an absent `crossorigin` keeps the HTML default `no-cors` + `include`
//! credential behavior.

use crate::cors_settings::{
    cors_request_settings, CorsCredentialsMode, CorsRequestMode, CorsSettingsAttribute,
};
use crate::cookie_same_site::SameSiteRequestContext;
use crate::document_referrer::DocumentReferrerContext;
use crate::navigation_network::{NavigationNetwork, NetworkCredentialsMode};
use crate::net::{FetchError, FetchRequest, FetchResponse, Origin};
use crate::subresource_cors::validate_subresource_cors_response;

impl DocumentReferrerContext {
    /// Fetch an HTML element subresource with request credential semantics.
    ///
    /// HTML distinguishes three request shapes:
    ///
    /// - missing `crossorigin`: `no-cors` + `include`; cross-origin cookies may
    ///   accompany the request, subject to normal cookie policy, and no `Origin`
    ///   header/CORS response gate is added here;
    /// - anonymous (including empty/invalid values): `cors` + `same-origin`;
    /// - `use-credentials`: `cors` + `include`.
    ///
    /// For CORS-mode requests the browser replaces any authored `Origin` header
    /// with the committed document origin. Cookie eligibility is recomputed for
    /// every redirect hop after HSTS and `Set-Cookie` acceptance follows the
    /// selected credential mode.
    pub fn fetch_subresource_with_cors_credentials(
        &self,
        network: &NavigationNetwork,
        request: &FetchRequest,
        context: SameSiteRequestContext,
        referrerpolicy: Option<&str>,
        crossorigin: Option<&str>,
    ) -> Result<FetchResponse, FetchError> {
        let settings = cors_request_settings(crossorigin);
        let source = self.source();
        let credentials_mode = match settings.credentials_mode {
            CorsCredentialsMode::SameOrigin => NetworkCredentialsMode::SameOrigin,
            CorsCredentialsMode::Include => NetworkCredentialsMode::Include,
        };

        if settings.mode == CorsRequestMode::NoCors {
            return network.fetch_with_referrer_and_credentials(
                request,
                context,
                Some(self.subresource_redirect_state(referrerpolicy)),
                credentials_mode,
                source,
            );
        }

        let source = source.ok_or_else(|| {
            FetchError::Blocked("CORS: CORS-enabled subresource has no request origin".into())
        })?;

        let mut cors_request = request.clone();
        cors_request.headers.delete("origin");
        cors_request
            .headers
            .insert_raw("origin", &Origin::of(source).header_value());

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
        let missing = cors_request_settings(None);
        assert_eq!(missing.mode, CorsRequestMode::NoCors);
        assert_eq!(missing.credentials_mode, CorsCredentialsMode::Include);
    }
}
