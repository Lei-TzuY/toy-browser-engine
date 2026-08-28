use browser_engine::cookie::CookieJar;
use browser_engine::document::Document;
use browser_engine::net::{MemoryLoader, Url};

fn run_js(html: &str, js: &str) -> Document {
    let loader = MemoryLoader::new();
    let url = Url::parse("http://example.com/app/index.html").unwrap();
    let full_html = format!(
        "<!DOCTYPE html><html><body>{}<script>{}</script></body></html>",
        html, js
    );
    Document::from_html(&full_html, &url, &loader)
}

#[test]
fn test_cookie_jar_rfc6265_basic_and_scoping() {
    let mut jar = CookieJar::new();
    let url1 = Url::parse("https://example.com/app/index.html").unwrap();
    let url2 = Url::parse("https://example.com/other/index.html").unwrap();
    let url_diff_domain = Url::parse("https://otherdomain.com/app/index.html").unwrap();

    // Set cookies via Set-Cookie header strings
    let c1 = CookieJar::parse_set_cookie("sessionId=xyz123; Path=/app; Secure; HttpOnly", &url1, 1000).unwrap();
    let c2 = CookieJar::parse_set_cookie("theme=dark; Path=/; Max-Age=3600", &url1, 1000).unwrap();
    let c3 = CookieJar::parse_set_cookie("temp=1; Path=/; Max-Age=0", &url1, 1000).unwrap();

    assert_eq!(c1.name, "sessionId");
    assert_eq!(c1.value, "xyz123");
    assert!(c1.http_only);
    assert!(c1.secure);
    assert_eq!(c1.path, "/app");

    jar.store(c1, 1000);
    jar.store(c2, 1000);
    jar.store(c3, 1000);

    // temp had Max-Age=0 so it should not be stored
    assert_eq!(jar.len(), 2);

    // get_document_cookie should hide HttpOnly cookie (sessionId) and only return theme=dark
    let doc_cookies = jar.get_document_cookie(&url1, 1000);
    assert_eq!(doc_cookies, "theme=dark");

    // HTTP cookie header includes HttpOnly cookies
    let http_cookies = jar.get_http_cookie_header(&url1, 1000).unwrap();
    assert!(http_cookies.contains("sessionId=xyz123"));
    assert!(http_cookies.contains("theme=dark"));

    // Path scoping: /other should match theme (Path=/) but NOT sessionId (Path=/app)
    let http_cookies_other = jar.get_http_cookie_header(&url2, 1000).unwrap();
    assert_eq!(http_cookies_other, "theme=dark");

    // Domain scoping: otherdomain.com should match nothing
    assert!(jar.get_http_cookie_header(&url_diff_domain, 1000).is_none());
}

#[test]
fn test_document_cookie_js_getter_and_setter() {
    let doc = run_js(
        r#"<div id="d"></div>"#,
        r#"
            // Set initial cookie
            document.cookie = "username=Alice; path=/";
            document.cookie = "role=admin; path=/app";

            console.log("cookies1:" + document.cookie);

            // Update cookie value
            document.cookie = "username=Bob; path=/";
            console.log("cookies2:" + document.cookie);

            // Delete cookie with max-age=0
            document.cookie = "role=admin; path=/app; max-age=0";
            console.log("cookies3:" + document.cookie);
        "#,
    );

    let logs = doc.runtime.console;
    assert!(logs[0].contains("username=Alice"));
    assert!(logs[0].contains("role=admin"));

    assert!(logs[1].contains("username=Bob"));
    assert!(logs[1].contains("role=admin"));

    assert_eq!(logs[2], "cookies3:username=Bob");
}
