use crate::net::{FetchError, FetchResponse, HeaderMap, Method};

/// Internal response marker written only by the single-hop redirect layer.
/// It carries the serialized Origin value against which the final CORS response
/// must be checked after a redirect chain. The runtime removes it before script
/// can observe response headers.
pub(crate) const CORS_REDIRECT_ORIGIN_HEADER: &str = "x-browser-internal-cors-origin";

pub(crate) fn is_cors_safelisted_method(method: Method) -> bool {
    matches!(method, Method::Get | Method::Head | Method::Post)
}

fn contains_cors_unsafe_request_header_byte(value: &str) -> bool {
    value.bytes().any(|byte| {
        (byte < 0x20 && byte != b'\t') || byte == 0x7f || b"\"():<>?@[\\]{}".contains(&byte)
    })
}

/// Parse Fetch's CORS-safelisted single byte-range form.
///
/// The safelist deliberately excludes suffix ranges such as `bytes=-500`, even
/// though they are valid HTTP Range values, because browsers historically did
/// not emit that shape from script-authored CORS-safelisted requests.
fn is_cors_safelisted_range_value(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("bytes=") else {
        return false;
    };
    let Some((start, end)) = rest.split_once('-') else {
        return false;
    };
    if start.is_empty() || !start.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    if !end.is_empty() && !end.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    if end.contains('-') {
        return false;
    }

    if end.is_empty() {
        return true;
    }

    fn normalized_decimal(value: &str) -> &str {
        let normalized = value.trim_start_matches('0');
        if normalized.is_empty() { "0" } else { normalized }
    }

    let start = normalized_decimal(start);
    let end = normalized_decimal(end);
    start.len() < end.len() || (start.len() == end.len() && start <= end)
}

fn is_cors_safelisted_request_header(name: &str, value: &str) -> bool {
    if value.len() > 128 {
        return false;
    }
    match name {
        "accept" => !contains_cors_unsafe_request_header_byte(value),
        "accept-language" | "content-language" => value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b" *,-.;=".contains(&byte)),
        "content-type" => {
            if contains_cors_unsafe_request_header_byte(value) {
                return false;
            }
            let mime = value
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            matches!(
                mime.as_str(),
                "application/x-www-form-urlencoded" | "multipart/form-data" | "text/plain"
            )
        }
        "range" => is_cors_safelisted_range_value(value),
        _ => false,
    }
}

/// Return authored request-header names that make a cross-origin request
/// non-simple. Browser-owned CORS fields are deliberately ignored.
pub(crate) fn cors_unsafe_request_header_names(headers: &HeaderMap) -> Vec<String> {
    let mut names = Vec::new();
    for (name, value) in headers.iter() {
        if matches!(
            name,
            "origin" | "access-control-request-method" | "access-control-request-headers"
        ) {
            continue;
        }
        if !is_cors_safelisted_request_header(name, value) {
            names.push(name.to_string());
        }
    }
    names.sort();
    names.dedup();
    names
}

/// Validate an actual CORS response against the serialized Origin value that
/// was sent for its request hop.
pub(crate) fn validate_cors_response_origin(
    serialized_origin: &str,
    credentialed: bool,
    response: &FetchResponse,
) -> Result<(), FetchError> {
    let allow_origin = response.headers.get("access-control-allow-origin");

    if credentialed {
        if allow_origin.as_deref() != Some(serialized_origin) {
            return Err(FetchError::Blocked(
                "CORS: credentialed response requires an exact Access-Control-Allow-Origin value"
                    .into(),
            ));
        }
        if response
            .headers
            .get("access-control-allow-credentials")
            .as_deref()
            != Some("true")
        {
            return Err(FetchError::Blocked(
                "CORS: credentialed response requires Access-Control-Allow-Credentials: true"
                    .into(),
            ));
        }
        return Ok(());
    }

    if matches!(allow_origin.as_deref(), Some("*"))
        || allow_origin.as_deref() == Some(serialized_origin)
    {
        Ok(())
    } else {
        Err(FetchError::Blocked(
            "CORS: cross-origin response did not allow the request origin".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cors_safelisted_range_accepts_single_forward_byte_ranges() {
        for value in ["bytes=0-0", "bytes=0-99", "bytes=42-"] {
            assert!(is_cors_safelisted_range_value(value), "{value}");
        }
    }

    #[test]
    fn cors_safelisted_range_handles_positions_larger_than_machine_integers() {
        let huge = "99999999999999999999999999999999999999999999999999";
        let same = format!("bytes={huge}-{huge}");
        assert!(same.len() <= 128);
        assert!(is_cors_safelisted_range_value(&same));

        let forward = format!("bytes=1-{huge}");
        assert!(is_cors_safelisted_range_value(&forward));

        let reversed = format!("bytes={huge}-1");
        assert!(!is_cors_safelisted_range_value(&reversed));
    }

    #[test]
    fn cors_safelisted_range_rejects_suffix_multi_and_reversed_ranges() {
        for value in [
            "bytes=-500",
            "bytes=100-99",
            "bytes=0-1,2-3",
            "bytes =0-1",
            "Bytes=0-1",
            "bytes=0 -1",
            "bytes=0- 1",
            "bytes=0-a",
        ] {
            assert!(!is_cors_safelisted_range_value(value), "{value}");
        }
    }

    #[test]
    fn range_safelist_respects_the_128_byte_header_value_limit() {
        let mut headers = HeaderMap::new();
        headers.insert_raw("range", "bytes=0-99");
        assert!(cors_unsafe_request_header_names(&headers).is_empty());

        let oversized = format!("bytes=1-{}", "9".repeat(121));
        assert!(oversized.len() > 128);
        headers.insert_raw("range", &oversized);
        assert_eq!(cors_unsafe_request_header_names(&headers), vec!["range"]);
    }
}
