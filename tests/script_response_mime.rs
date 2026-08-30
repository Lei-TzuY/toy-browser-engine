use browser_engine::{
    is_javascript_mime_type, script_response_is_allowed, script_response_mime_disposition,
    ScriptResponseMimeDisposition,
};

#[test]
fn legacy_javascript_mime_essences_are_recognized() {
    for mime in [
        "text/javascript",
        "application/javascript",
        "application/ecmascript",
        "text/ecmascript",
        "text/javascript1.5",
        "text/jscript",
        "text/livescript",
        "text/x-javascript",
    ] {
        assert!(is_javascript_mime_type(mime), "{mime}");
    }
}

#[test]
fn parameters_and_ascii_case_do_not_change_javascript_mime_identity() {
    assert!(is_javascript_mime_type(
        " APPLICATION/JAVASCRIPT ; charset=utf-8"
    ));
    assert!(is_javascript_mime_type("Text/JavaScript;foo=bar"));
}

#[test]
fn script_destination_blocks_fetch_incompatible_media_groups() {
    for mime in ["image/svg+xml", "audio/mpeg", "video/webm", "text/csv"] {
        assert_eq!(
            script_response_mime_disposition(Some(mime), None),
            ScriptResponseMimeDisposition::Block,
            "{mime}"
        );
    }
}

#[test]
fn script_destination_without_nosniff_preserves_legacy_compatibility() {
    for mime in ["text/plain", "application/octet-stream", "application/json"] {
        assert!(script_response_is_allowed(Some(mime), None), "{mime}");
    }
    assert!(script_response_is_allowed(None, None));
}

#[test]
fn nosniff_turns_the_check_into_a_javascript_mime_allowlist() {
    assert!(script_response_is_allowed(
        Some("text/javascript; charset=utf-8"),
        Some("nosniff")
    ));
    assert!(!script_response_is_allowed(
        Some("text/plain"),
        Some("nosniff")
    ));
    assert!(!script_response_is_allowed(
        Some("application/json"),
        Some("NOSNIFF")
    ));
    assert!(!script_response_is_allowed(None, Some("nosniff")));
}

#[test]
fn malformed_content_type_is_allowed_without_nosniff_but_blocked_with_it() {
    assert!(script_response_is_allowed(Some("not a mime type"), None));
    assert!(!script_response_is_allowed(
        Some("not a mime type"),
        Some("nosniff")
    ));
}

#[test]
fn only_the_first_x_content_type_options_list_member_controls_nosniff() {
    assert!(!script_response_is_allowed(
        Some("text/plain"),
        Some(" nosniff , other")
    ));
    assert!(script_response_is_allowed(
        Some("text/plain"),
        Some("other, nosniff")
    ));
}
