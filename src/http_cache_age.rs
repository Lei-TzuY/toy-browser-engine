//! RFC 9111 HTTP cache age calculation primitives.
//!
//! The browser's cache policy already models freshness lifetime, but deciding
//! whether an entry is fresh also requires a conservative estimate of its
//! current age. This module keeps that arithmetic independent from a concrete
//! cache store and from HTTP-date parsing.

use crate::net::fetch::HeaderMap;

/// Timing inputs used by RFC 9111 section 4.2.3 age calculation.
///
/// All values are whole seconds. `request_time_secs`, `response_time_secs`,
/// `now_secs`, and `date_value_secs` must be expressed in the same clock domain.
/// A caller that cannot reliably parse or compare the response `Date` value can
/// pass `None`, in which case apparent age contributes zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpCacheAgeInput {
    pub date_value_secs: Option<u64>,
    pub request_time_secs: u64,
    pub response_time_secs: u64,
    pub now_secs: u64,
}

/// Parse the response `Age` field as delta-seconds.
///
/// RFC 9111 defines `Age` as a singleton field, but when a list is received a
/// cache should use the first member and discard the rest. Invalid first members
/// are ignored and therefore contribute an age value of zero.
pub fn response_age_value_secs(headers: &HeaderMap) -> u64 {
    let Some(value) = headers.get("age") else {
        return 0;
    };

    let first = value.split(',').next().unwrap_or_default().trim();
    if first.is_empty() || !first.bytes().all(|b| b.is_ascii_digit()) {
        return 0;
    }

    first.parse::<u64>().unwrap_or(u64::MAX)
}

/// Compute the corrected initial age of a response when it enters the cache.
///
/// This uses the conservative RFC 9111 form:
/// `max(apparent_age, age_value + response_delay)`.
pub fn corrected_initial_age_secs(
    headers: &HeaderMap,
    input: HttpCacheAgeInput,
) -> u64 {
    let apparent_age = input
        .date_value_secs
        .map(|date| input.response_time_secs.saturating_sub(date))
        .unwrap_or(0);

    let response_delay = input
        .response_time_secs
        .saturating_sub(input.request_time_secs);
    let corrected_age_value = response_age_value_secs(headers).saturating_add(response_delay);

    apparent_age.max(corrected_age_value)
}

/// Compute the current age of a stored response.
///
/// Resident time is measured from response receipt until `now`; backwards clock
/// movement is clamped to zero so it can never make an entry younger.
pub fn current_age_secs(headers: &HeaderMap, input: HttpCacheAgeInput) -> u64 {
    let corrected_initial_age = corrected_initial_age_secs(headers, input);
    let resident_time = input.now_secs.saturating_sub(input.response_time_secs);
    corrected_initial_age.saturating_add(resident_time)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(age: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(age) = age {
            headers.insert_raw("age", age);
        }
        headers
    }

    fn input(date_value_secs: Option<u64>) -> HttpCacheAgeInput {
        HttpCacheAgeInput {
            date_value_secs,
            request_time_secs: 100,
            response_time_secs: 110,
            now_secs: 140,
        }
    }

    #[test]
    fn age_header_uses_first_list_member() {
        assert_eq!(response_age_value_secs(&headers(Some("12, 99"))), 12);
    }

    #[test]
    fn invalid_age_header_is_ignored() {
        for age in ["", "-1", "+1", "1.5", "bogus", "bogus, 10"] {
            assert_eq!(response_age_value_secs(&headers(Some(age))), 0, "{age}");
        }
    }

    #[test]
    fn missing_age_header_contributes_zero() {
        assert_eq!(response_age_value_secs(&headers(None)), 0);
    }

    #[test]
    fn apparent_age_uses_response_time_minus_date() {
        let age = corrected_initial_age_secs(&headers(None), input(Some(80)));
        assert_eq!(age, 30);
    }

    #[test]
    fn future_date_clamps_apparent_age_to_zero() {
        let age = corrected_initial_age_secs(&headers(None), input(Some(200)));
        assert_eq!(age, 10);
    }

    #[test]
    fn corrected_age_value_includes_response_delay() {
        let age = corrected_initial_age_secs(&headers(Some("25")), input(Some(105)));
        assert_eq!(age, 35);
    }

    #[test]
    fn conservative_initial_age_takes_the_larger_estimate() {
        let age = corrected_initial_age_secs(&headers(Some("5")), input(Some(60)));
        assert_eq!(age, 50);
    }

    #[test]
    fn current_age_adds_resident_time() {
        let age = current_age_secs(&headers(Some("25")), input(Some(105)));
        assert_eq!(age, 65);
    }

    #[test]
    fn backwards_local_timing_never_underflows() {
        let input = HttpCacheAgeInput {
            date_value_secs: None,
            request_time_secs: 150,
            response_time_secs: 140,
            now_secs: 130,
        };
        assert_eq!(current_age_secs(&headers(None), input), 0);
    }

    #[test]
    fn arithmetic_saturates_instead_of_wrapping() {
        let input = HttpCacheAgeInput {
            date_value_secs: None,
            request_time_secs: 0,
            response_time_secs: u64::MAX,
            now_secs: u64::MAX,
        };
        assert_eq!(current_age_secs(&headers(Some("1")), input), u64::MAX);
    }
}
