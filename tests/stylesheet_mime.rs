use browser_engine::{
    stylesheet_mime_disposition, stylesheet_response_is_css, StylesheetMimeDisposition,
};

#[test]
fn stylesheet_links_accept_css_response_metadata() {
    assert_eq!(
        stylesheet_mime_disposition(Some("text/css; charset=utf-8")),
        StylesheetMimeDisposition::Apply
    );
    assert!(stylesheet_response_is_css(Some("TEXT/CSS")));
}

#[test]
fn stylesheet_links_ignore_valid_non_css_response_types() {
    assert_eq!(
        stylesheet_mime_disposition(Some("text/plain")),
        StylesheetMimeDisposition::Ignore
    );
    assert_eq!(
        stylesheet_mime_disposition(Some("application/octet-stream")),
        StylesheetMimeDisposition::Ignore
    );
}

#[test]
fn missing_or_invalid_metadata_uses_html_stylesheet_default() {
    assert_eq!(
        stylesheet_mime_disposition(None),
        StylesheetMimeDisposition::Apply
    );
    assert_eq!(
        stylesheet_mime_disposition(Some("null")),
        StylesheetMimeDisposition::Apply
    );
    assert_eq!(
        stylesheet_mime_disposition(Some("\"text/css\"")),
        StylesheetMimeDisposition::Apply
    );
}

#[test]
fn media_type_parameters_do_not_change_the_css_essence() {
    assert!(stylesheet_response_is_css(Some(
        " text/css ; charset=\"utf-8\""
    )));
    assert!(!stylesheet_response_is_css(Some(
        "text/html; charset=utf-8"
    )));
}
