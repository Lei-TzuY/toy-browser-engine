use browser_engine::page_icon::{
    discover_page_icon_candidates, IconSizeHint, PageIconResolver,
};
use browser_engine::{Browser, MemoryLoader, Url};

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

fn png_backed_ico(width: u8, height: u8, rgba: [u8; 4]) -> Vec<u8> {
    let payload = png_rgba(u32::from(width), u32::from(height), rgba);
    let mut out = vec![0u8; 22];
    out[2..4].copy_from_slice(&1u16.to_le_bytes());
    out[4..6].copy_from_slice(&1u16.to_le_bytes());
    out[6] = width;
    out[7] = height;
    out[10..12].copy_from_slice(&1u16.to_le_bytes());
    out[12..14].copy_from_slice(&32u16.to_le_bytes());
    out[14..18].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    out[18..22].copy_from_slice(&22u32.to_le_bytes());
    out.extend_from_slice(&payload);
    out
}

fn open(html: &str, resources: Vec<(&str, Vec<u8>)>) -> Browser {
    let mut loader = MemoryLoader::new();
    loader.insert("demo:///pages/index.html", html.as_bytes().to_vec());
    for (url, bytes) in resources {
        loader.insert(url, bytes);
    }
    Browser::open(
        Box::new(loader),
        &Url::parse("demo:///pages/index.html").unwrap(),
    )
    .unwrap()
}

#[test]
fn exact_declared_size_wins_and_decodes_through_image_stack() {
    let browser = open(
        "<head>\
           <link rel='icon' href='/icons/small.png' sizes='16x16'>\
           <link rel='icon' href='/icons/exact.ico' sizes='32x32'>\
           <link rel='icon' href='/icons/large.png' sizes='64x64'>\
         </head>",
        vec![
            ("demo:///icons/small.png", png_rgba(16, 16, [255, 0, 0, 255])),
            ("demo:///icons/exact.ico", png_backed_ico(32, 32, [0, 255, 0, 200])),
            ("demo:///icons/large.png", png_rgba(64, 64, [0, 0, 255, 255])),
        ],
    );

    let candidates = discover_page_icon_candidates(browser.document());
    assert_eq!(candidates.len(), 3);
    assert_eq!(
        candidates[1].sizes,
        vec![IconSizeHint::Pixels { width: 32, height: 32 }]
    );

    let mut resolver = PageIconResolver::new();
    let result = resolver.resolve(&browser, 32, 32);
    let icon = result.icon.expect("resolved exact icon");
    assert_eq!(icon.source.to_string(), "demo:///icons/exact.ico");
    assert_eq!((icon.image.width, icon.image.height), (32, 32));
    assert_eq!(icon.image.pixel(0, 0), [0, 255, 0, 200]);
    assert!(!icon.is_legacy_fallback());
    assert_eq!(result.report.discovered, 3);
    assert_eq!(result.report.attempted, 1);
    assert_eq!(result.report.failed, 0);
}

#[test]
fn broken_preferred_candidate_falls_through_to_next_ranked_icon() {
    let browser = open(
        "<head>\
           <link rel='icon' href='/bad.ico' sizes='32x32'>\
           <link rel='icon' href='/good.png' sizes='64x64'>\
         </head>",
        vec![
            ("demo:///bad.ico", b"broken icon".to_vec()),
            ("demo:///good.png", png_rgba(64, 64, [9, 8, 7, 255])),
        ],
    );

    let mut resolver = PageIconResolver::new();
    let result = resolver.resolve(&browser, 32, 32);
    let icon = result.icon.expect("fallback explicit icon");
    assert_eq!(icon.source.to_string(), "demo:///good.png");
    assert_eq!(icon.image.pixel(0, 0), [9, 8, 7, 255]);
    assert_eq!(result.report.attempted, 2);
    assert_eq!(result.report.failed, 1);
    assert!(!result.report.legacy_fallback_attempted);

    // Both the failed and successful decode are memoized; resolving again does
    // not grow the cache or retry broken bytes.
    assert_eq!(resolver.cache().len(), 2);
    let second = resolver.resolve(&browser, 32, 32);
    assert!(second.icon.is_some());
    assert_eq!(resolver.cache().len(), 2);
}

#[test]
fn relative_icon_uses_document_base_url() {
    let browser = open(
        "<head>\
           <base href='demo:///assets/theme/'>\
           <link rel='icon' href='fav.png' sizes='any'>\
         </head>",
        vec![(
            "demo:///assets/theme/fav.png",
            png_rgba(2, 2, [11, 22, 33, 255]),
        )],
    );

    let mut resolver = PageIconResolver::new();
    let result = resolver.resolve(&browser, 32, 32);
    let icon = result.icon.expect("base-relative icon");
    assert_eq!(icon.source.to_string(), "demo:///assets/theme/fav.png");
    assert_eq!(icon.image.pixel(1, 1), [11, 22, 33, 255]);
}

#[test]
fn no_explicit_icon_uses_legacy_root_favicon() {
    let browser = open(
        "<head><title>legacy</title></head>",
        vec![(
            "demo:///favicon.ico",
            png_backed_ico(16, 16, [70, 80, 90, 255]),
        )],
    );

    let mut resolver = PageIconResolver::new();
    let result = resolver.resolve(&browser, 16, 16);
    let icon = result.icon.expect("legacy favicon");
    assert!(icon.is_legacy_fallback());
    assert_eq!(icon.source.to_string(), "demo:///favicon.ico");
    assert_eq!(icon.image.pixel(0, 0), [70, 80, 90, 255]);
    assert_eq!(result.report.discovered, 0);
    assert_eq!(result.report.attempted, 1);
    assert!(result.report.legacy_fallback_attempted);
}

#[test]
fn legacy_fallback_can_be_disabled() {
    let browser = open(
        "<head><title>no icon</title></head>",
        vec![(
            "demo:///favicon.ico",
            png_backed_ico(16, 16, [1, 2, 3, 255]),
        )],
    );

    let mut resolver = PageIconResolver::new();
    resolver.set_legacy_fallback(false);
    let result = resolver.resolve(&browser, 16, 16);
    assert!(result.icon.is_none());
    assert_eq!(result.report.attempted, 0);
    assert!(!result.report.legacy_fallback_attempted);
    assert!(resolver.cache().is_empty());
}
