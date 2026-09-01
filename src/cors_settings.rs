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

/// Fetch request mode selected by an HTML CORS settings attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorsRequestMode {
    NoCors,
    Cors,
}

/// Credential mode selected when fetching an HTML subresource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorsCredentialsMode {
    SameOrigin,
    Include,
}

/// Complete Fetch-facing request settings implied by an HTML `crossorigin`
/// attribute.
///
/// The missing-attribute case matters: ordinary element loads are `no-cors`
/// requests with credentials included, while anonymous CORS switches to
/// `cors` + `same-origin` credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorsRequestSettings {
    pub mode: CorsRequestMode,
    pub credentials_mode: CorsCredentialsMode,
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

/// Resolve the complete Fetch request semantics for an HTML CORS settings
/// attribute.
///
/// HTML's potentially-CORS-enabled fetch behavior distinguishes the absent
/// attribute from the anonymous state:
///
/// - missing: `no-cors` + `include`
/// - anonymous / empty / invalid: `cors` + `same-origin`
/// - use-credentials: `cors` + `include`
pub fn cors_request_settings(value: Option<&str>) -> CorsRequestSettings {
    match parse_cors_settings_attribute(value) {
        None => CorsRequestSettings {
            mode: CorsRequestMode::NoCors,
            credentials_mode: CorsCredentialsMode::Include,
        },
        Some(setting) => CorsRequestSettings {
            mode: CorsRequestMode::Cors,
            credentials_mode: setting.credentials_mode(),
        },
    }
}

/// Whether the presence/value of `crossorigin` enables a CORS-mode request.
///
/// Any present value enables CORS because invalid and empty values resolve to
/// the Anonymous state. Only a missing attribute leaves the element in its
/// non-CORS request mode.
pub fn cors_enabled(value: Option<&str>) -> bool {
    matches!(cors_request_settings(value).mode, CorsRequestMode::Cors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_attribute_means_no_cors() {
        assert_eq!(parse_cors_settings_attribute(None), None);
        assert!(!cors_enabled(None));
        assert_eq!(
            cors_request_settings(None),
            CorsRequestSettings {
                mode: CorsRequestMode::NoCors,
                credentials_mode: CorsCredentialsMode::Include,
            }
        );
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
        for value in [Some("anonymous"), Some("")] {
            assert_eq!(
                cors_request_settings(value),
                CorsRequestSettings {
                    mode: CorsRequestMode::Cors,
                    credentials_mode: CorsCredentialsMode::SameOrigin,
                }
            );
        }
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
        assert_eq!(
            cors_request_settings(Some("USE-CREDENTIALS")),
            CorsRequestSettings {
                mode: CorsRequestMode::Cors,
                credentials_mode: CorsCredentialsMode::Include,
            }
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
            assert_eq!(
                cors_request_settings(Some(value)),
                CorsRequestSettings {
                    mode: CorsRequestMode::Cors,
                    credentials_mode: CorsCredentialsMode::SameOrigin,
                }
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
