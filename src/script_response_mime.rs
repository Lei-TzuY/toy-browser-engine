//! Fetch/HTML response MIME policy for script-like destinations.
//!
//! Classic script fetching has two related but distinct MIME checks in Fetch:
//! the generic script-destination blocklist, and the stricter
//! `X-Content-Type-Options: nosniff` check. Keeping those rules together here
//! makes it harder for element loaders to accidentally implement only one half.

/// The result of applying Fetch's script-response MIME gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptResponseMimeDisposition {
    Allow,
    Block,
}

/// Return whether a parsed MIME type is one of the JavaScript MIME types
/// recognized by the MIME Sniffing standard.
pub fn is_javascript_mime_type(content_type: &str) -> bool {
    let Some(essence) = mime_essence(content_type) else {
        return false;
    };

    matches!(
        essence.as_str(),
        "application/ecmascript"
            | "application/javascript"
            | "application/x-ecmascript"
            | "application/x-javascript"
            | "text/ecmascript"
            | "text/javascript"
            | "text/javascript1.0"
            | "text/javascript1.1"
            | "text/javascript1.2"
            | "text/javascript1.3"
            | "text/javascript1.4"
            | "text/javascript1.5"
            | "text/jscript"
            | "text/livescript"
            | "text/x-ecmascript"
            | "text/x-javascript"
    )
}

/// Apply Fetch's MIME response blocking rules for a script-like destination.
///
/// Without `nosniff`, Fetch only blocks a small set of clearly incompatible
/// supplied MIME types for script destinations (audio, image, video and CSV).
/// With `nosniff`, a missing/invalid Content-Type or any non-JavaScript MIME
/// type is blocked.
pub fn script_response_mime_disposition(
    content_type: Option<&str>,
    x_content_type_options: Option<&str>,
) -> ScriptResponseMimeDisposition {
    let nosniff = determines_nosniff(x_content_type_options);
    let parsed = content_type.and_then(mime_essence);

    if nosniff {
        return match parsed {
            Some(ref essence) if is_javascript_mime_essence(essence) => {
                ScriptResponseMimeDisposition::Allow
            }
            _ => ScriptResponseMimeDisposition::Block,
        };
    }

    match parsed {
        None => ScriptResponseMimeDisposition::Allow,
        Some(essence)
            if essence.starts_with("audio/")
                || essence.starts_with("image/")
                || essence.starts_with("video/")
                || essence == "text/csv" =>
        {
            ScriptResponseMimeDisposition::Block
        }
        Some(_) => ScriptResponseMimeDisposition::Allow,
    }
}

/// Convenience predicate for element/network loader call sites.
pub fn script_response_is_allowed(
    content_type: Option<&str>,
    x_content_type_options: Option<&str>,
) -> bool {
    script_response_mime_disposition(content_type, x_content_type_options)
        == ScriptResponseMimeDisposition::Allow
}

fn determines_nosniff(value: Option<&str>) -> bool {
    value
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .is_some_and(|value| value.eq_ignore_ascii_case("nosniff"))
}

fn is_javascript_mime_essence(essence: &str) -> bool {
    matches!(
        essence,
        "application/ecmascript"
            | "application/javascript"
            | "application/x-ecmascript"
            | "application/x-javascript"
            | "text/ecmascript"
            | "text/javascript"
            | "text/javascript1.0"
            | "text/javascript1.1"
            | "text/javascript1.2"
            | "text/javascript1.3"
            | "text/javascript1.4"
            | "text/javascript1.5"
            | "text/jscript"
            | "text/livescript"
            | "text/x-ecmascript"
            | "text/x-javascript"
    )
}

fn mime_essence(input: &str) -> Option<String> {
    let essence = input.split(';').next()?.trim();
    let (type_, subtype) = essence.split_once('/')?;
    if type_.is_empty() || subtype.is_empty() || subtype.contains('/') {
        return None;
    }
    if !type_.bytes().all(is_http_token_byte) || !subtype.bytes().all(is_http_token_byte) {
        return None;
    }
    Some(format!(
        "{}/{}",
        type_.to_ascii_lowercase(),
        subtype.to_ascii_lowercase()
    ))
}

fn is_http_token_byte(byte: u8) -> bool {
    matches!(
        byte,
        b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^'
            | b'_' | b'`' | b'|' | b'~'
            | b'0'..=b'9'
            | b'A'..=b'Z'
            | b'a'..=b'z'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn javascript_mime_types_are_case_insensitive_and_ignore_parameters() {
        assert!(is_javascript_mime_type("text/javascript"));
        assert!(is_javascript_mime_type("TEXT/JAVASCRIPT; charset=utf-8"));
        assert!(is_javascript_mime_type("application/x-javascript"));
        assert!(is_javascript_mime_type("text/livescript"));
        assert!(!is_javascript_mime_type("text/plain"));
    }

    #[test]
    fn ordinary_script_fetch_blocks_known_incompatible_mime_groups() {
        for content_type in [
            "image/png",
            "audio/ogg",
            "video/mp4",
            "text/csv; charset=utf-8",
        ] {
            assert_eq!(
                script_response_mime_disposition(Some(content_type), None),
                ScriptResponseMimeDisposition::Block,
                "{content_type}"
            );
        }
    }

    #[test]
    fn ordinary_script_fetch_keeps_legacy_non_blocklisted_types_compatible() {
        assert!(script_response_is_allowed(Some("text/plain"), None));
        assert!(script_response_is_allowed(Some("application/octet-stream"), None));
        assert!(script_response_is_allowed(None, None));
        assert!(script_response_is_allowed(Some("not a mime type"), None));
    }

    #[test]
    fn nosniff_requires_a_javascript_mime_type() {
        assert!(script_response_is_allowed(
            Some("text/javascript; charset=utf-8"),
            Some("nosniff")
        ));
        assert!(!script_response_is_allowed(Some("text/plain"), Some("nosniff")));
        assert!(!script_response_is_allowed(None, Some("nosniff")));
        assert!(!script_response_is_allowed(
            Some("not a mime type"),
            Some("nosniff")
        ));
    }

    #[test]
    fn nosniff_token_is_case_insensitive_but_only_first_list_member_controls() {
        assert!(!script_response_is_allowed(
            Some("text/plain"),
            Some("NoSnIfF")
        ));
        assert!(script_response_is_allowed(
            Some("text/plain"),
            Some("other, nosniff")
        ));
    }
}
