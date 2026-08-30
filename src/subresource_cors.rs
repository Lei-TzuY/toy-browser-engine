//! CORS response validation for element-initiated subresource fetches.
//!
//! HTML's `crossorigin` attribute decides whether an element uses CORS and,
//! when it does, whether credentials are permitted. This module keeps that
//! policy separate from transport: callers can validate the final response of
//! an image/script/stylesheet-style fetch without teaching the low-level HTTP
//! client about DOM attributes.

use crate::cors_settings::{parse_cors_settings_attribute, CorsSettingsAttribute};
use crate::net::{FetchError, FetchResponse, Origin, Url};

/// Validate the final response of an element-initiated request that carries an
/// HTML CORS settings attribute.
///
/// A missing `crossorigin` attribute keeps the element in its existing no-CORS
/// path and therefore performs no CORS response-header check here. For a
/// present attribute, same-origin responses are accepted directly. A
/// cross-origin response must satisfy the CORS protocol:
///
/// - Anonymous requests accept an exact `Access-Control-Allow-Origin` value or
///   `*`.
/// - `use-credentials` requires an exact origin match and
///   `Access-Control-Allow-Credentials: true`; wildcard origins are rejected.
///
/// Repeated ACAO/ACAC fields are conservatively rejected because `HeaderMap`
/// joins repeated values with `, ` and those combined values cannot equal one
/// of the protocol's permitted singleton values.
pub fn validate_subresource_cors_response(
    source: Option<&Url>,
    crossorigin: Option<&str>,
    response: &FetchResponse,
) -> Result<(), FetchError> {
    let Some(setting) = parse_cors_settings_attribute(crossorigin) else {
        return Ok(());
    };

    let Some(source) = source else {
        return Err(cors_blocked(
            "CORS-enabled subresource has no request origin",
        ));
    };

    if Origin::of(source) == Origin::of(&response.url) {
        return Ok(());
    }

    let serialized_origin = Origin::of(source).header_value();
    let allow_origin = response.headers.get("access-control-allow-origin");

    match setting {
        CorsSettingsAttribute::Anonymous => {
            if matches!(allow_origin.as_deref(), Some("*"))
                || allow_origin.as_deref() == Some(serialized_origin.as_str())
            {
                Ok(())
            } else {
                Err(cors_blocked(
                    "cross-origin response did not allow the document origin",
                ))
            }
        }
        CorsSettingsAttribute::UseCredentials => {
            if allow_origin.as_deref() != Some(serialized_origin.as_str()) {
                return Err(cors_blocked(
                    "credentialed CORS requires an exact Access-Control-Allow-Origin value",
                ));
            }

            if response
                .headers
                .get("access-control-allow-credentials")
                .as_deref()
                != Some("true")
            {
                return Err(cors_blocked(
                    "credentialed CORS requires Access-Control-Allow-Credentials: true",
                ));
            }

            Ok(())
        }
    }
}

fn cors_blocked(reason: &str) -> FetchError {
    FetchError::Blocked(format!("CORS: {reason}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::FetchResponse;

    fn url(input: &str) -> Url {
        Url::parse(input).unwrap()
    }

    fn response(url_value: &str) -> FetchResponse {
        FetchResponse::synthetic(url(url_value), 200, Some("text/plain"), Vec::new())
    }

    #[test]
    fn missing_crossorigin_keeps_existing_no_cors_path() {
        let response = response("https://cdn.test/image.png");
        assert!(validate_subresource_cors_response(
            Some(&url("https://page.test/index.html")),
            None,
            &response,
        )
        .is_ok());
    }

    #[test]
    fn same_origin_cors_request_needs_no_allow_origin_header() {
        let response = response("https://page.test/image.png");
        assert!(validate_subresource_cors_response(
            Some(&url("https://page.test/index.html")),
            Some("anonymous"),
            &response,
        )
        .is_ok());
    }

    #[test]
    fn anonymous_cross_origin_accepts_wildcard() {
        let mut response = response("https://cdn.test/image.png");
        response
            .headers
            .insert_raw("access-control-allow-origin", "*");

        assert!(validate_subresource_cors_response(
            Some(&url("https://page.test/index.html")),
            Some("anonymous"),
            &response,
        )
        .is_ok());
    }

    #[test]
    fn credentialed_cross_origin_rejects_wildcard() {
        let mut response = response("https://cdn.test/image.png");
        response
            .headers
            .insert_raw("access-control-allow-origin", "*");
        response
            .headers
            .insert_raw("access-control-allow-credentials", "true");

        assert!(validate_subresource_cors_response(
            Some(&url("https://page.test/index.html")),
            Some("use-credentials"),
            &response,
        )
        .is_err());
    }

    #[test]
    fn credentialed_cross_origin_requires_exact_origin_and_true() {
        let mut response = response("https://cdn.test/image.png");
        response
            .headers
            .insert_raw("access-control-allow-origin", "https://page.test");
        response
            .headers
            .insert_raw("access-control-allow-credentials", "true");

        assert!(validate_subresource_cors_response(
            Some(&url("https://page.test/index.html")),
            Some("use-credentials"),
            &response,
        )
        .is_ok());
    }

    #[test]
    fn credentials_true_is_case_sensitive() {
        let mut response = response("https://cdn.test/image.png");
        response
            .headers
            .insert_raw("access-control-allow-origin", "https://page.test");
        response
            .headers
            .insert_raw("access-control-allow-credentials", "TRUE");

        assert!(validate_subresource_cors_response(
            Some(&url("https://page.test/index.html")),
            Some("use-credentials"),
            &response,
        )
        .is_err());
    }

    #[test]
    fn repeated_allow_origin_fields_fail_singleton_match() {
        let mut response = response("https://cdn.test/image.png");
        response
            .headers
            .append_raw("access-control-allow-origin", "https://page.test");
        response
            .headers
            .append_raw("access-control-allow-origin", "https://page.test");

        assert!(validate_subresource_cors_response(
            Some(&url("https://page.test/index.html")),
            Some("anonymous"),
            &response,
        )
        .is_err());
    }
}
