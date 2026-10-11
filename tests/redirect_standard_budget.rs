use browser_engine::net::{FetchRequest, FetchResponse, Url};
use browser_engine::{RedirectError, RedirectPlanner, FETCH_MAX_REDIRECTS};

fn redirect(from: &Url) -> FetchResponse {
    let mut response = FetchResponse::synthetic(from.clone(), 302, None, Vec::new());
    response.headers.insert_raw("location", "/next");
    response
}

#[test]
fn default_redirect_planner_accepts_exactly_twenty_hops() {
    let mut planner = RedirectPlanner::default();
    let mut request = FetchRequest::get(Url::parse("https://example.test/start").unwrap());

    assert_eq!(planner.max_redirects(), FETCH_MAX_REDIRECTS);
    assert_eq!(planner.remaining(), FETCH_MAX_REDIRECTS);

    for accepted in 1..=FETCH_MAX_REDIRECTS {
        let response = redirect(&request.url);
        request = planner
            .next_request(&request, &response)
            .unwrap()
            .expect("302 with Location should produce the next request");
        assert_eq!(planner.followed(), accepted);
        assert_eq!(planner.remaining(), FETCH_MAX_REDIRECTS - accepted);
    }

    let response = redirect(&request.url);
    assert_eq!(
        planner.next_request(&request, &response),
        Err(RedirectError::TooManyRedirects(request.url.to_string()))
    );
    assert_eq!(planner.followed(), FETCH_MAX_REDIRECTS);
    assert_eq!(planner.remaining(), 0);
}

#[test]
fn custom_redirect_budget_remains_available_for_embedders() {
    let mut planner = RedirectPlanner::new(2);
    let mut request = FetchRequest::get(Url::parse("https://example.test/start").unwrap());

    for _ in 0..2 {
        let response = redirect(&request.url);
        request = planner.next_request(&request, &response).unwrap().unwrap();
    }

    assert_eq!(planner.max_redirects(), 2);
    assert_eq!(planner.followed(), 2);
    assert_eq!(planner.remaining(), 0);
}
