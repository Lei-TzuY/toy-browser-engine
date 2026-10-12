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

pub(crate) fn is_cors_safelisted_request_header(name: &str, value: &str) -> bool {
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
        "range" => is_cors_safelisted_range(value),
        _ => false,
    }
}

fn is_cors_safelisted_range(value: &str) -> bool {
    let Some(range) = value.strip_prefix("bytes=") else {
        return false;
    };
    let Some((start, end)) = range.split_once('-') else {
        return false;
    };
    !start.is_empty()
        && start.bytes().all(|byte| byte.is_ascii_digit())
        && end.bytes().all(|byte| byte.is_ascii_digit())
}

const CORS_SAFELIST_VALUE_SIZE_LIMIT: usize = 1024;

pub(crate) fn cors_unsafe_request_header_names(headers: &HeaderMap) -> Vec<String> {
    let mut unsafe_names = Vec::new();
    let mut potentially_unsafe_names = Vec::new();
    let mut safelist_value_size = 0usize;

    for (name, value) in headers.iter() {
        // Origin is browser-owned and is inserted after this calculation for
        // ordinary CORS requests. Keep the exclusion as defense-in-depth for
        // internally constructed requests that already carry one.
        if matches!(
            name,
            "origin" | "access-control-request-method" | "access-control-request-headers"
        ) {
            continue;
        }

        if is_cors_safelisted_request_header(name, value) {
            potentially_unsafe_names.push(name.to_ascii_lowercase());
            safelist_value_size = safelist_value_size.saturating_add(value.len());
        } else {
            unsafe_names.push(name.to_ascii_lowercase());
        }
    }

    // Fetch promotes every otherwise-safelisted header name when the sum of
    // their individual header-value byte lengths exceeds 1024 bytes.
    if safelist_value_size > CORS_SAFELIST_VALUE_SIZE_LIMIT {
        unsafe_names.extend(potentially_unsafe_names);
    }

    unsafe_names.sort();
    unsafe_names.dedup();
    unsafe_names
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
