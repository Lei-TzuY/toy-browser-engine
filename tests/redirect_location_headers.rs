use browser_engine::net::{FetchRequest, FetchResponse, Url};
use browser_engine::{RedirectError, RedirectPlanner};

fn url(input: &str) -> Url {
    Url::parse(input).unwrap()
}

fn redirect(locations: &[&str]) -> FetchResponse {
    let mut response =
        FetchResponse::synthetic(url("https://example.test/start"), 302, None, Vec::new());
    for location in locations {
        response.headers.append_raw("location", location);
    }
    response
}

#[test]
fn duplicate_location_fields_fail_instead_of_becoming_a_combined_relative_url() {
    let request = FetchRequest::get(url("https://example.test/start"));
    let mut planner = RedirectPlanner::default();

    assert_eq!(
        planner.next_request(&request, &redirect(&["/safe", "https://other.test/evil"])),
        Err(RedirectError::InvalidLocation(
            "multiple Location header fields".to_string()
        ))
    );
    assert_eq!(planner.followed(), 0);
}

#[test]
fn repeated_identical_location_fields_are_still_ambiguous() {
    let request = FetchRequest::get(url("https://example.test/start"));
    let mut planner = RedirectPlanner::default();

    assert!(matches!(
        planner.next_request(&request, &redirect(&["/next", "/next"])),
        Err(RedirectError::InvalidLocation(_))
    ));
    assert_eq!(planner.followed(), 0);
}

#[test]
fn comma_in_one_location_field_remains_part_of_the_uri_reference() {
    let request = FetchRequest::get(url("https://example.test/start"));
    let mut planner = RedirectPlanner::new(1);
    let next = planner
        .next_request(&request, &redirect(&["/search?q=one,two"]))
        .unwrap()
        .unwrap();

    assert_eq!(
        next.url.to_string(),
        "https://example.test/search?q=one,two"
    );
    assert_eq!(planner.followed(), 1);
}

#[test]
fn duplicate_location_failure_precedes_zero_redirect_budget() {
    let request = FetchRequest::get(url("https://example.test/start"));
    let mut planner = RedirectPlanner::new(0);

    assert!(matches!(
        planner.next_request(&request, &redirect(&["/one", "/two"])),
        Err(RedirectError::InvalidLocation(_))
    ));
    assert_eq!(planner.followed(), 0);
}
