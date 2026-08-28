use browser_engine::browser::Browser;
use browser_engine::image::{CursorCache, ImageCache};
use browser_engine::net::{decode_data_url, DefaultLoader, MemoryLoader, ResourceLoader, Url};
use browser_engine::page_icon::PageIconResolver;

const PNG_DATA: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGPgUbI4AQAB0wEvEZKPrQAAAABJRU5ErkJggg==";

#[test]
fn public_decoder_handles_binary_and_text_data_urls() {
    let text = Url::parse("data:text/plain;charset=UTF-8,hello%20data%21").unwrap();
    let decoded = decode_data_url(&text).unwrap();
    assert_eq!(decoded.mime, "text/plain");
    assert_eq!(decoded.bytes, b"hello data!");

    let png = decode_data_url(&Url::parse(PNG_DATA).unwrap()).unwrap();
    assert_eq!(png.mime, "image/png");
    assert!(png.bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
}

#[test]
fn image_and_cursor_caches_share_the_data_loader_path() {
    let loader = DefaultLoader::new();
    let url = Url::parse(PNG_DATA).unwrap();

    let mut images = ImageCache::new();
    let image = images.fetch(&url, &loader).unwrap();
    assert_eq!((image.width, image.height), (1, 1));
    assert_eq!(image.pixel(0, 0), [12, 34, 56, 200]);

    let mut cursors = CursorCache::new();
    let cursor = cursors.fetch(&url, &loader).unwrap();
    assert_eq!(cursor.hotspot(), (0, 0));
    assert_eq!(cursor.image.pixel(0, 0), [12, 34, 56, 200]);
}

#[test]
fn document_img_and_page_icon_can_use_inline_data_images() {
    let mut memory = MemoryLoader::new();
    memory.insert(
        "demo:///index.html",
        format!(
            r#"<!doctype html>
               <title>inline assets</title>
               <link rel="icon" sizes="1x1" href="{PNG_DATA}">
               <img id="inline" src="{PNG_DATA}">"#
        ),
    );
    let loader = DefaultLoader::new().with_memory(memory);
    let browser = Browser::open(
        Box::new(loader),
        &Url::parse("demo:///index.html").unwrap(),
    )
    .unwrap();

    let image_url = Url::parse(PNG_DATA).unwrap();
    let image = browser.document().images.get(&image_url).unwrap();
    assert_eq!(image.pixel(0, 0), [12, 34, 56, 200]);

    let mut icons = PageIconResolver::new();
    icons.set_legacy_fallback(false);
    let resolution = icons.resolve(&browser, 1, 1);
    let icon = resolution.icon.expect("data favicon resolves");
    assert_eq!(icon.image.pixel(0, 0), [12, 34, 56, 200]);
    assert_eq!(resolution.report.failed, 0);
}

#[test]
fn default_loader_serves_data_urls_without_registration() {
    let loader = DefaultLoader::new();
    let url = Url::parse("data:application/octet-stream;base64,AAEC/w==").unwrap();
    let resource = loader.load(&url).unwrap();
    assert_eq!(resource.effective_mime(), "application/octet-stream");
    assert_eq!(resource.bytes, vec![0, 1, 2, 255]);
}
