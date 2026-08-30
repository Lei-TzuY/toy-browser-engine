//! HTTP `Vary` parsing and cache-selection helpers.
//!
//! A cached response with `Vary: *` cannot be selected for reuse. Otherwise,
//! each listed request-header field participates in the cache key: the stored
//! request and the candidate request must have equivalent values for every
//! selected field.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HttpVary {
    Any,
    Fields(Vec<String>),
}

/// Parse all `Vary` response-header field lines.
///
/// Field names are canonicalized to lowercase and de-duplicated while keeping
/// first-seen order. Invalid field names fail closed as `HttpVary::Any` so a
/// malformed response cannot accidentally widen cache reuse.
pub fn parse_vary(headers: &[(String, String)]) -> HttpVary {
    let mut fields = Vec::new();

    for (name, value) in headers {
        if !name.eq_ignore_ascii_case("vary") {
            continue;
        }

        for member in value.split(',') {
            let member = member.trim_matches(|c| c == ' ' || c == '\t');
            if member.is_empty() {
                continue;
            }
            if member == "*" {
                return HttpVary::Any;
            }
            if !is_field_name(member) {
                return HttpVary::Any;
            }

            let canonical = member.to_ascii_lowercase();
            if !fields.iter().any(|existing| existing == &canonical) {
                fields.push(canonical);
            }
        }
    }

    HttpVary::Fields(fields)
}

/// Return whether a cached response may be selected for `candidate_headers`
/// according to its `Vary` response header and the request headers that were
/// used when it was stored.
pub fn vary_matches(
    response_headers: &[(String, String)],
    stored_request_headers: &[(String, String)],
    candidate_headers: &[(String, String)],
) -> bool {
    match parse_vary(response_headers) {
        HttpVary::Any => false,
        HttpVary::Fields(fields) => fields.into_iter().all(|field| {
            normalized_field_value(stored_request_headers, &field)
                == normalized_field_value(candidate_headers, &field)
        }),
    }
}

fn normalized_field_value(headers: &[(String, String)], field: &str) -> Option<String> {
    let values: Vec<&str> = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case(field))
        .map(|(_, value)| value.trim_matches(|c| c == ' ' || c == '\t'))
        .collect();

    if values.is_empty() {
        None
    } else {
        Some(values.join(","))
    }
}

fn is_field_name(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(is_tchar)
}

fn is_tchar(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^'
                | b'_' | b'`' | b'|' | b'~'
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn parses_multiple_lines_and_deduplicates_case_insensitively() {
        let vary = parse_vary(&headers(&[("Vary", "Accept-Encoding, Accept-Language"), ("vary", "accept-encoding")]));
        assert_eq!(
            vary,
            HttpVary::Fields(vec!["accept-encoding".into(), "accept-language".into()])
        );
    }

    #[test]
    fn wildcard_never_matches() {
        assert!(!vary_matches(
            &headers(&[("Vary", "*")]),
            &[],
            &[]
        ));
    }

    #[test]
    fn matching_is_header_name_case_insensitive() {
        assert!(vary_matches(
            &headers(&[("Vary", "Accept-Language")]),
            &headers(&[("accept-language", "en-US")]),
            &headers(&[("ACCEPT-LANGUAGE", "en-US")])
        ));
    }

    #[test]
    fn changed_selected_header_rejects_reuse() {
        assert!(!vary_matches(
            &headers(&[("Vary", "Accept-Language")]),
            &headers(&[("Accept-Language", "en-US")]),
            &headers(&[("Accept-Language", "fr")])
        ));
    }

    #[test]
    fn absent_header_matches_absent_header() {
        assert!(vary_matches(
            &headers(&[("Vary", "Accept-Language")]),
            &[],
            &[]
        ));
    }

    #[test]
    fn absent_and_present_headers_do_not_match() {
        assert!(!vary_matches(
            &headers(&[("Vary", "Accept-Language")]),
            &[],
            &headers(&[("Accept-Language", "en")])
        ));
    }

    #[test]
    fn duplicate_request_fields_are_combined_in_order() {
        let response = headers(&[("Vary", "Accept")]);
        let stored = headers(&[("Accept", "text/html"), ("accept", " application/json ")]);
        let same = headers(&[("ACCEPT", " text/html "), ("Accept", "application/json")]);
        let reversed = headers(&[("Accept", "application/json"), ("Accept", "text/html")]);
        assert!(vary_matches(&response, &stored, &same));
        assert!(!vary_matches(&response, &stored, &reversed));
    }

    #[test]
    fn malformed_vary_fails_closed() {
        assert_eq!(parse_vary(&headers(&[("Vary", "Accept Language")])), HttpVary::Any);
    }
}
