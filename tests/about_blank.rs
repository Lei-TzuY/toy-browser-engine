use browser_engine::net::{
    AboutLoader, DefaultLoader, FetchError, FetchRequest, LoadError, MemoryLoader, ResourceLoader,
    Url,
};
use browser_engine::{Browser, PointerState};

fn about_blank() -> Url {
    Url::parse("about:blank").unwrap()
}

#[test]
fn about_blank_loads_as_a_minimal_html_document() {
    let url = about_blank();
    assert!(url.is_opaque());
    assert_eq!(url.to_string(), "about:blank");

    let resource = DefaultLoader::new().load(&url).unwrap();
    assert_eq!(resource.url, url);
    assert_eq!(resource.effective_mime(), "text/html");
    assert!(resource.text().contains("<body>"));
}

#[test]
fn browser_can_open_reload_and_render_about_blank() {
    let url = about_blank();
    let mut browser = Browser::open(Box::new(DefaultLoader::new()), &url).unwrap();

    assert_eq!(browser.url().to_string(), "about:blank");
    assert_eq!(browser.document().url.to_string(), "about:blank");
    assert_eq!(browser.history().len(), 1);

    browser.reload().unwrap();
    assert_eq!(browser.url().to_string(), "about:blank");

    // Rendering the built-in document should need no special frontend path.
    let canvas = browser.render(320, 200, 0.0, &PointerState::default());
    assert_eq!(canvas.width, 320);
    assert_eq!(canvas.height, 200);
}

#[test]
fn history_moves_between_normal_pages_and_about_blank() {
    let mut memory = MemoryLoader::new();
    memory.insert(
        "demo:///index.html",
        "<!doctype html><html><head><title>Demo</title></head><body><p>demo</p></body></html>",
    );
    let loader = DefaultLoader::new().with_memory(memory);
    let demo = Url::parse("demo:///index.html").unwrap();
    let blank = about_blank();
    let mut browser = Browser::open(Box::new(loader), &demo).unwrap();

    browser.navigate(&blank).unwrap();
    assert_eq!(browser.url().to_string(), "about:blank");
    assert_eq!(browser.history().len(), 2);
    assert!(browser.can_go_back());

    assert!(browser.back());
    assert_eq!(browser.url().to_string(), "demo:///index.html");
    assert_eq!(browser.document().url.to_string(), "demo:///index.html");

    assert!(browser.forward());
    assert_eq!(browser.url().to_string(), "about:blank");
    assert_eq!(browser.document().url.to_string(), "about:blank");
}

#[test]
fn about_blank_query_and_fragment_are_valid_history_urls() {
    let url = Url::parse("about:blank?debug=1#section").unwrap();
    let mut browser = Browser::open(Box::new(DefaultLoader::new()), &url).unwrap();
    assert_eq!(browser.url().to_string(), "about:blank?debug=1#section");
    assert_eq!(browser.document().url.to_string(), "about:blank?debug=1#section");
    browser.reload().unwrap();
    assert_eq!(browser.url().to_string(), "about:blank?debug=1#section");
}

#[test]
fn unknown_about_pages_are_not_document_resources() {
    let url = Url::parse("about:config").unwrap();
    assert!(matches!(
        AboutLoader.load(&url),
        Err(LoadError::NotFound(_))
    ));
    assert!(Browser::open(Box::new(DefaultLoader::new()), &url).is_err());
}

#[test]
fn about_blank_is_not_exposed_as_a_fetch_endpoint() {
    let request = FetchRequest::get(about_blank());
    assert!(matches!(
        DefaultLoader::new().fetch(&request),
        Err(FetchError::UnsupportedScheme(scheme)) if scheme == "about"
    ));
}
