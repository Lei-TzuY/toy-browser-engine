use browser_engine::{
    cors_enabled, parse_cors_settings_attribute, CorsCredentialsMode, CorsSettingsAttribute,
};

#[test]
fn missing_crossorigin_keeps_element_out_of_cors_mode() {
    assert_eq!(parse_cors_settings_attribute(None), None);
    assert!(!cors_enabled(None));
}

#[test]
fn present_empty_or_invalid_crossorigin_uses_anonymous_cors() {
    for value in ["", "anonymous", "invalid", " use-credentials "] {
        assert_eq!(
            parse_cors_settings_attribute(Some(value)),
            Some(CorsSettingsAttribute::Anonymous),
            "unexpected state for {value:?}"
        );
        assert!(cors_enabled(Some(value)));
    }
}

#[test]
fn use_credentials_selects_include_credentials() {
    let state = parse_cors_settings_attribute(Some("Use-Credentials"));
    assert_eq!(state, Some(CorsSettingsAttribute::UseCredentials));
    assert_eq!(
        state.unwrap().credentials_mode(),
        CorsCredentialsMode::Include
    );
}

#[test]
fn anonymous_never_escalates_to_include_credentials() {
    let state = parse_cors_settings_attribute(Some("ANONYMOUS")).unwrap();
    assert_eq!(state.credentials_mode(), CorsCredentialsMode::SameOrigin);
}

#[test]
fn whitespace_around_use_credentials_falls_back_to_anonymous() {
    assert_eq!(
        parse_cors_settings_attribute(Some("\tuse-credentials\n")),
        Some(CorsSettingsAttribute::Anonymous)
    );
}
