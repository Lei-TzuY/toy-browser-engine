// ============================================================
//  cors_settings.rs — HTML CORS settings attribute semantics
// ============================================================

/// Parsed state of an HTML CORS settings attribute such as `crossorigin`.
///
/// HTML defines two keyword states. A missing attribute means "No CORS" and
/// is therefore represented by `None` from [`parse_cors_settings_attribute`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorsSettingsAttribute {
    Anonymous,
    UseCredentials,
}

/// Credential mode selected when a CORS settings attribute enables CORS.
///
/// This intentionally models the HTML attribute's effect without tying it to
/// one particular network backend. A later fetch integration can translate
/// these values to its own request representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorsCredentialsMode {
    SameOrigin,
    Include,
}

impl CorsSettingsAttribute {
    /// The Fetch credentials mode implied by this CORS settings state.
    pub fn credentials_mode(self) -> CorsCredentialsMode {
        match self {
            Self::Anonymous => CorsCredentialsMode::SameOrigin,
            Self::UseCredentials => CorsCredentialsMode::Include,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anonymous => "anonymous",
            Self::UseCredentials => "use-credentials",
        }
    }
}

/// Parse an HTML CORS settings attribute (`crossorigin`).
///
/// The attribute is an HTML enumerated attribute with:
///
/// - `anonymous` -> Anonymous
/// - `use-credentials` -> UseCredentials
/// - the empty value -> Anonymous
/// - any invalid value -> Anonymous
/// - a missing attribute -> No CORS (`None`)
///
/// Keyword matching is ASCII-case-insensitive. We deliberately do not trim
/// whitespace before matching: whitespace-surrounded `use-credentials` is an
/// invalid value and therefore falls back to Anonymous rather than silently
/// enabling credentials.
pub fn parse_cors_settings_attribute(value: Option<&str>) -> Option<CorsSettingsAttribute> {
    let value = value?;
    if value.eq_ignore_ascii_case("use-credentials") {
        Some(CorsSettingsAttribute::UseCredentials)
    } else {
        // `anonymous`, the empty-value default, and the invalid-value default
        // all resolve to the Anonymous state.
        Some(CorsSettingsAttribute::Anonymous)
    }
}

/// Whether the presence/value of `crossorigin` enables a CORS-mode request.
///
/// Any present value enables CORS because invalid and empty values resolve to
/// the Anonymous state. Only a missing attribute leaves the element in its
/// non-CORS request mode.
pub fn cors_enabled(value: Option<&str>) -> bool {
    parse_cors_settings_attribute(value).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_attribute_means_no_cors() {
        assert_eq!(parse_cors_settings_attribute(None), None);
        assert!(!cors_enabled(None));
    }

    #[test]
    fn anonymous_and_empty_values_select_anonymous_state() {
        assert_eq!(
            parse_cors_settings_attribute(Some("anonymous")),
            Some(CorsSettingsAttribute::Anonymous)
        );
        assert_eq!(
            parse_cors_settings_attribute(Some("")),
            Some(CorsSettingsAttribute::Anonymous)
        );
    }

    #[test]
    fn use_credentials_is_ascii_case_insensitive() {
        assert_eq!(
            parse_cors_settings_attribute(Some("USE-CREDENTIALS")),
            Some(CorsSettingsAttribute::UseCredentials)
        );
        assert_eq!(
            CorsSettingsAttribute::UseCredentials.credentials_mode(),
            CorsCredentialsMode::Include
        );
    }

    #[test]
    fn invalid_values_use_anonymous_default() {
        for value in ["credentialed", " anonymous ", " use-credentials ", "future-mode"] {
            assert_eq!(
                parse_cors_settings_attribute(Some(value)),
                Some(CorsSettingsAttribute::Anonymous),
                "unexpected parse for {value:?}"
            );
        }
    }

    #[test]
    fn anonymous_uses_same_origin_credentials_mode() {
        assert_eq!(
            CorsSettingsAttribute::Anonymous.credentials_mode(),
            CorsCredentialsMode::SameOrigin
        );
    }

    #[test]
    fn any_present_attribute_enables_cors() {
        assert!(cors_enabled(Some("anonymous")));
        assert!(cors_enabled(Some("use-credentials")));
        assert!(cors_enabled(Some("")));
        assert!(cors_enabled(Some("invalid")));
    }
}
