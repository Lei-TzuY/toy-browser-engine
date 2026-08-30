use browser_engine::net::{FetchRequest, FetchResponse, Url};
use browser_engine::{RedirectError, RedirectPlanner};

fn redirect(location: &str) -> FetchResponse {
    let mut response = FetchResponse::synthetic(
        Url::parse("https://example.test/start").unwrap(),
        302,
        None,
        Vec::new(),
    );
    response.headers.insert_raw("location", location);
    response
}

#[test]
fn redirect_planner_rejects_non_http_fetch_schemes() {
    let request = FetchRequest::get(Url::parse("https://example.test/start").unwrap());

    for (location, expected_scheme) in [
        ("file:///tmp/next", "file"),
        ("data:text/plain,hello", "data"),
        ("about:blank", "about"),
    ] {
        let mut planner = RedirectPlanner::new(5);
        assert_eq!(
            planner.next_request(&request, &redirect(location)),
            Err(RedirectError::UnsupportedScheme(
                expected_scheme.to_string()
            ))
        );
        assert_eq!(planner.followed(), 0);
    }
}

#[test]
fn redirect_planner_accepts_http_and_https_targets() {
    let request = FetchRequest::get(Url::parse("https://example.test/start").unwrap());

    for location in ["http://other.test/next", "https://other.test/next"] {
        let mut planner = RedirectPlanner::new(1);
        let next = planner
            .next_request(&request, &redirect(location))
            .unwrap()
            .unwrap();
        assert_eq!(next.url.to_string(), location);
        assert_eq!(planner.followed(), 1);
    }
}

#[test]
fn unsupported_scheme_error_precedes_redirect_budget_error() {
    let request = FetchRequest::get(Url::parse("https://example.test/start").unwrap());
    let mut planner = RedirectPlanner::new(0);

    assert_eq!(
        planner.next_request(&request, &redirect("file:///tmp/next")),
        Err(RedirectError::UnsupportedScheme("file".to_string()))
    );
    assert_eq!(planner.followed(), 0);
}
