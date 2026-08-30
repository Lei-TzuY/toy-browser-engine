//! HTML stylesheet response MIME handling.
//!
//! The `stylesheet` link type has a CSS default type. A syntactically valid
//! response `Content-Type` therefore controls whether a fetched resource can be
//! applied as CSS, while missing or syntactically invalid metadata falls back
//! to the link type's `text/css` default.

/// Whether a fetched external stylesheet response may be parsed as CSS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StylesheetMimeDisposition {
    Apply,
    Ignore,
}

/// Classify response `Content-Type` metadata for an HTML stylesheet link.
///
/// HTML gives stylesheet links a default type of `text/css`. Consequently:
///
/// - no `Content-Type` metadata => apply as CSS;
/// - syntactically invalid metadata => fall back to the CSS default;
/// - valid `text/css` metadata (case-insensitive, parameters allowed) => apply;
/// - any other valid media type => ignore the resource as a stylesheet.
///
/// This intentionally models the HTML stylesheet-type decision only. Fetch's
/// separate `X-Content-Type-Options: nosniff` request blocking belongs at the
/// request/response policy layer and is not folded into this helper.
pub fn stylesheet_mime_disposition(content_type: Option<&str>) -> StylesheetMimeDisposition {
    let Some(value) = content_type else {
        return StylesheetMimeDisposition::Apply;
    };
    let Some((type_, subtype)) = parse_mime_essence(value) else {
        return StylesheetMimeDisposition::Apply;
    };

    if type_.eq_ignore_ascii_case("text") && subtype.eq_ignore_ascii_case("css") {
        StylesheetMimeDisposition::Apply
    } else {
        StylesheetMimeDisposition::Ignore
    }
}

/// Convenience predicate for callers that only need the apply/ignore answer.
pub fn stylesheet_response_is_css(content_type: Option<&str>) -> bool {
    stylesheet_mime_disposition(content_type) == StylesheetMimeDisposition::Apply
}

/// Parse just enough of an HTTP media type to establish its MIME essence.
///
/// Parameters are deliberately ignored after the essence. Invalid essence
/// syntax is reported as `None`, which lets the HTML layer use its stylesheet
/// default type rather than accidentally treating malformed metadata as a
/// different, valid media type.
fn parse_mime_essence(value: &str) -> Option<(&str, &str)> {
    let essence = value.split_once(';').map_or(value, |(head, _)| head).trim();
    let (type_, subtype) = essence.split_once('/')?;
    let type_ = type_.trim();
    let subtype = subtype.trim();

    if type_.is_empty()
        || subtype.is_empty()
        || subtype.contains('/')
        || !type_.bytes().all(is_http_token_byte)
        || !subtype.bytes().all(is_http_token_byte)
    {
        return None;
    }

    Some((type_, subtype))
}

fn is_http_token_byte(byte: u8) -> bool {
    matches!(
        byte,
        b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^'
            | b'_' | b'`' | b'|' | b'~' | b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_content_type_uses_stylesheet_default() {
        assert_eq!(
            stylesheet_mime_disposition(None),
            StylesheetMimeDisposition::Apply
        );
    }

    #[test]
    fn text_css_is_accepted_case_insensitively_with_parameters() {
        for value in [
            "text/css",
            "TEXT/CSS",
            " text/css ",
            "text/css; charset=utf-8",
            "Text/Css ; charset=\"utf-8\"",
        ] {
            assert_eq!(
                stylesheet_mime_disposition(Some(value)),
                StylesheetMimeDisposition::Apply,
                "{value:?}"
            );
        }
    }

    #[test]
    fn other_valid_media_types_are_ignored() {
        for value in ["text/plain", "text/html", "application/octet-stream"] {
            assert_eq!(
                stylesheet_mime_disposition(Some(value)),
                StylesheetMimeDisposition::Ignore,
                "{value:?}"
            );
        }
    }

    #[test]
    fn malformed_metadata_falls_back_to_css_default() {
        for value in ["", "null", "\"text/css\"", "text/", "/css", "text/css/extra"] {
            assert_eq!(
                stylesheet_mime_disposition(Some(value)),
                StylesheetMimeDisposition::Apply,
                "{value:?}"
            );
        }
    }

    #[test]
    fn convenience_predicate_matches_disposition() {
        assert!(stylesheet_response_is_css(Some("text/css")));
        assert!(stylesheet_response_is_css(None));
        assert!(!stylesheet_response_is_css(Some("text/plain")));
    }
}
