use browser_engine::{
    corrected_initial_age_secs, current_age_secs, response_age_value_secs, HttpCacheAgeInput,
};
use browser_engine::net::fetch::HeaderMap;

fn headers(age: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(age) = age {
        headers.insert_raw("age", age);
    }
    headers
}

#[test]
fn age_header_uses_first_member_and_ignores_invalid_values() {
    assert_eq!(response_age_value_secs(&headers(Some("7, 99"))), 7);
    assert_eq!(response_age_value_secs(&headers(Some("invalid, 99"))), 0);
    assert_eq!(response_age_value_secs(&headers(None)), 0);
}

#[test]
fn corrected_initial_age_includes_network_delay() {
    let input = HttpCacheAgeInput {
        date_value_secs: Some(1_000),
        request_time_secs: 1_020,
        response_time_secs: 1_025,
        now_secs: 1_025,
    };

    assert_eq!(corrected_initial_age_secs(&headers(Some("30")), input), 35);
}

#[test]
fn apparent_age_can_dominate_forwarded_age() {
    let input = HttpCacheAgeInput {
        date_value_secs: Some(900),
        request_time_secs: 1_000,
        response_time_secs: 1_010,
        now_secs: 1_010,
    };

    assert_eq!(corrected_initial_age_secs(&headers(Some("20")), input), 110);
}

#[test]
fn current_age_adds_cache_resident_time() {
    let input = HttpCacheAgeInput {
        date_value_secs: Some(995),
        request_time_secs: 1_000,
        response_time_secs: 1_010,
        now_secs: 1_050,
    };

    assert_eq!(current_age_secs(&headers(Some("20")), input), 70);
}

#[test]
fn future_date_and_backwards_clock_movement_are_clamped() {
    let input = HttpCacheAgeInput {
        date_value_secs: Some(2_000),
        request_time_secs: 1_500,
        response_time_secs: 1_400,
        now_secs: 1_300,
    };

    assert_eq!(current_age_secs(&headers(None), input), 0);
}
