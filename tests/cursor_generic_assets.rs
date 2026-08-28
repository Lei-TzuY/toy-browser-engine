use browser_engine::cursor_assets::{CursorResolver, ResolvedCursor};
use browser_engine::image::{decode_cursor_asset, CursorCache};
use browser_engine::script::dom_api;
use browser_engine::{Browser, MemoryLoader, PointerState, Url};

fn png_rgba(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..width * height {
            pixels.extend_from_slice(&rgba);
        }
        writer.write_image_data(&pixels).unwrap();
    }
    out
}

#[test]
fn public_cursor_asset_decoder_accepts_plain_png() {
    let cursor = decode_cursor_asset(&png_rgba(2, 1, [11, 22, 33, 44])).unwrap();
    assert_eq!(cursor.hotspot(), (0, 0));
    assert_eq!((cursor.image.width, cursor.image.height), (2, 1));
    assert_eq!(cursor.image.pixel(1, 0), [11, 22, 33, 44]);
}

#[test]
fn css_cursor_resolver_loads_plain_png_asset() {
    let mut loader = MemoryLoader::new();
    loader.insert(
        "demo:///index.html",
        b"<style>#target { cursor: url(pointer.png); }</style><div id='target'>target</div>"
            .to_vec(),
    );
    loader.insert("demo:///pointer.png", png_rgba(2, 2, [7, 8, 9, 180]));
    let browser = Browser::open(
        Box::new(loader),
        &Url::parse("demo:///index.html").unwrap(),
    )
    .unwrap();
    let target = dom_api::query_selector(&browser.document().dom, &[], "#target").unwrap();

    let mut resolver = CursorResolver::new();
    let resolved = resolver
        .resolve_for_path(&browser, &target, 800.0, &PointerState::default())
        .unwrap();
    match resolved {
        ResolvedCursor::Image { cursor, source, .. } => {
            assert_eq!(cursor.hotspot(), (0, 0));
            assert_eq!(cursor.image.pixel(0, 0), [7, 8, 9, 180]);
            assert_eq!(source.to_string(), "demo:///pointer.png");
        }
        ResolvedCursor::System(_) => panic!("expected image cursor"),
    }
}

#[test]
fn generic_cursor_cache_keeps_failure_entries() {
    let mut loader = MemoryLoader::new();
    loader.insert("demo:///bad.dat", b"broken".to_vec());
    let url = Url::parse("demo:///bad.dat").unwrap();
    let mut cache = CursorCache::new();

    assert!(cache.fetch(&url, &loader).is_err());
    assert!(cache.error(&url).is_some());
    assert_eq!(cache.len(), 1);
    assert!(cache.fetch(&url, &loader).is_err());
    assert_eq!(cache.len(), 1);
}
