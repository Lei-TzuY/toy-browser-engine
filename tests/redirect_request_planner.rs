use browser_engine::net::{FetchRequest, FetchResponse, HeaderMap, Method, Url};
use browser_engine::{RedirectError, RedirectPlanner};

fn response(url: &str, status: u16, location: Option<&str>) -> FetchResponse {
    let mut response = FetchResponse::synthetic(Url::parse(url).unwrap(), status, None, Vec::new());
    if let Some(location) = location {
        response.headers.insert_raw("location", location);
    }
    response
}

#[test]
fn planner_exposes_fetch_redirect_status_set_only() {
    let request = FetchRequest::get(Url::parse("http://example.test/a").unwrap());
    for status in [301, 302, 303, 307, 308] {
        let mut planner = RedirectPlanner::new(1);
        assert!(planner
            .next_request(&request, &response("http://example.test/a", status, Some("/b")))
            .unwrap()
            .is_some());
    }
    for status in [200, 300, 304, 305, 306, 309] {
        let mut planner = RedirectPlanner::new(1);
        assert!(planner
            .next_request(&request, &response("http://example.test/a", status, Some("/b")))
            .unwrap()
            .is_none());
    }
}

#[test]
fn status_303_preserves_head_but_rewrites_put_to_get() {
    let head = FetchRequest::new(
        Url::parse("http://example.test/a").unwrap(),
        Method::Head,
        HeaderMap::new(),
        None,
    );
    let mut planner = RedirectPlanner::new(2);
    let head_next = planner
        .next_request(&head, &response("http://example.test/a", 303, Some("/b")))
        .unwrap()
        .unwrap();
    assert_eq!(head_next.method, Method::Head);

    let mut headers = HeaderMap::new();
    headers.insert_raw("content-type", "application/json");
    headers.insert_raw("content-language", "en");
    let put = FetchRequest::new(
        Url::parse("http://example.test/b").unwrap(),
        Method::Put,
        headers,
        Some(br#"{"ok":true}"#.to_vec()),
    );
    let put_next = planner
        .next_request(&put, &response("http://example.test/b", 303, Some("/c")))
        .unwrap()
        .unwrap();
    assert_eq!(put_next.method, Method::Get);
    assert_eq!(put_next.body, None);
    assert!(!put_next.headers.has("content-type"));
    assert!(!put_next.headers.has("content-language"));
}

#[test]
fn status_301_and_302_only_rewrite_post() {
    for status in [301, 302] {
        let put = FetchRequest::new(
            Url::parse("http://example.test/a").unwrap(),
            Method::Put,
            HeaderMap::new(),
            Some(b"body".to_vec()),
        );
        let mut planner = RedirectPlanner::new(1);
        let next = planner
            .next_request(&put, &response("http://example.test/a", status, Some("/b")))
            .unwrap()
            .unwrap();
        assert_eq!(next.method, Method::Put);
        assert_eq!(next.body.as_deref(), Some(&b"body"[..]));
    }
}

#[test]
fn same_origin_default_port_keeps_authorization_but_cross_port_drops_it() {
    let mut headers = HeaderMap::new();
    headers.insert_raw("authorization", "Bearer secret");
    let request = FetchRequest::new(
        Url::parse("http://example.test:80/a").unwrap(),
        Method::Get,
        headers,
        None,
    );

    let mut planner = RedirectPlanner::new(2);
    let same = planner
        .next_request(
            &request,
            &response("http://example.test:80/a", 302, Some("http://EXAMPLE.test/b")),
        )
        .unwrap()
        .unwrap();
    assert_eq!(same.headers.get("authorization").as_deref(), Some("Bearer secret"));

    let cross_port = planner
        .next_request(
            &same,
            &response("http://example.test/b", 302, Some("http://example.test:8080/c")),
        )
        .unwrap()
        .unwrap();
    assert!(!cross_port.headers.has("authorization"));
}

#[test]
fn cookie_is_dropped_even_for_same_origin_same_path_redirect() {
    let mut headers = HeaderMap::new();
    headers.insert_raw("cookie", "sid=selected-for-old-hop");
    let request = FetchRequest::new(
        Url::parse("http://example.test/a").unwrap(),
        Method::Get,
        headers,
        None,
    );
    let mut planner = RedirectPlanner::new(1);
    let next = planner
        .next_request(&request, &response("http://example.test/a", 307, Some("/a?next=1")))
        .unwrap()
        .unwrap();
    assert!(!next.headers.has("cookie"));
}

#[test]
fn referer_is_dropped_on_every_redirect_for_policy_recomputation() {
    for location in ["/same-origin", "https://other.test/cross-origin"] {
        let mut headers = HeaderMap::new();
        headers.insert_raw("referer", "https://source.test/private/path?q=secret");
        headers.insert_raw("x-keep", "yes");
        let request = FetchRequest::new(
            Url::parse("https://example.test/start").unwrap(),
            Method::Get,
            headers,
            None,
        );
        let mut planner = RedirectPlanner::new(1);
        let next = planner
            .next_request(
                &request,
                &response("https://example.test/start", 302, Some(location)),
            )
            .unwrap()
            .unwrap();

        assert!(!next.headers.has("referer"));
        assert_eq!(next.headers.get("x-keep").as_deref(), Some("yes"));
    }
}

#[test]
fn zero_budget_rejects_first_real_redirect_but_not_ordinary_response() {
    let request = FetchRequest::get(Url::parse("http://example.test/a").unwrap());
    let mut planner = RedirectPlanner::new(0);
    assert_eq!(
        planner
            .next_request(&request, &response("http://example.test/a", 200, None))
            .unwrap(),
        None
    );
    assert_eq!(planner.followed(), 0);
    assert_eq!(
        planner.next_request(
            &request,
            &response("http://example.test/a", 302, Some("/b"))
        ),
        Err(RedirectError::TooManyRedirects(
            "http://example.test/a".to_string()
        ))
    );
}

#[test]
fn location_without_fragment_inherits_current_request_fragment() {
    let request = FetchRequest::get(
        Url::parse("https://example.test/start#section-2").unwrap(),
    );
    let mut planner = RedirectPlanner::new(1);
    let next = planner
        .next_request(
            &request,
            &response("https://example.test/start", 302, Some("/next?q=1")),
        )
        .unwrap()
        .unwrap();

    assert_eq!(
        next.url.to_string(),
        "https://example.test/next?q=1#section-2"
    );
}

#[test]
fn explicit_location_fragment_replaces_inherited_fragment() {
    let request = FetchRequest::get(Url::parse("https://example.test/start#old").unwrap());
    let mut planner = RedirectPlanner::new(1);
    let next = planner
        .next_request(
            &request,
            &response("https://example.test/start", 302, Some("/next#new")),
        )
        .unwrap()
        .unwrap();

    assert_eq!(next.url.to_string(), "https://example.test/next#new");
}
