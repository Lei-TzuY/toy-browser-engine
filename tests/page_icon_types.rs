use browser_engine::browser::Browser;
use browser_engine::net::{DefaultLoader, MemoryLoader, Url};
use browser_engine::page_icon::{icon_type_support, IconTypeSupport, PageIconResolver};

fn png_rgba(rgba: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&rgba).unwrap();
    }
    out
}

fn browser(html: &str, resources: &[(&str, Vec<u8>)]) -> Browser {
    let mut memory = MemoryLoader::new();
    memory.insert("demo:///index.html", html.as_bytes().to_vec());
    for (url, bytes) in resources {
        memory.insert(*url, bytes.clone());
    }
    Browser::open(
        Box::new(DefaultLoader::new().with_memory(memory)),
        &Url::parse("demo:///index.html").unwrap(),
    )
    .unwrap()
}

#[test]
fn supported_declared_type_beats_exact_size_known_unsupported_type() {
    let page = browser(
        r#"<head>
             <link rel="icon" href="exact.svg" sizes="32x32" type="image/svg+xml">
             <link rel="icon" href="larger.png" sizes="64x64" type="IMAGE/PNG; charset=binary">
           </head>"#,
        &[
            ("demo:///exact.svg", png_rgba([200, 10, 20, 255])),
            ("demo:///larger.png", png_rgba([10, 20, 200, 255])),
        ],
    );

    let mut resolver = PageIconResolver::new();
    resolver.set_legacy_fallback(false);
    let resolution = resolver.resolve(&page, 32, 32);
    let icon = resolution.icon.expect("supported MIME candidate resolves first");

    assert_eq!(icon.source.to_string(), "demo:///larger.png");
    assert_eq!(icon.image.pixel(0, 0), [10, 20, 200, 255]);
    assert_eq!(resolution.report.attempted, 1);
}

#[test]
fn mismatched_unsupported_type_is_still_sniffed_when_it_is_the_only_candidate() {
    let page = browser(
        r#"<head><link rel="icon" href="actually.png" sizes="32x32" type="image/gif"></head>"#,
        &[("demo:///actually.png", png_rgba([1, 2, 3, 240]))],
    );

    let mut resolver = PageIconResolver::new();
    resolver.set_legacy_fallback(false);
    let resolution = resolver.resolve(&page, 32, 32);
    let icon = resolution.icon.expect("actual PNG bytes remain usable despite bad hint");

    assert_eq!(icon.source.to_string(), "demo:///actually.png");
    assert_eq!(icon.image.pixel(0, 0), [1, 2, 3, 240]);
    assert_eq!(resolution.report.failed, 0);
}

#[test]
fn unknown_image_subtypes_rank_between_supported_and_known_unsupported() {
    assert!(IconTypeSupport::Supported < IconTypeSupport::UnspecifiedOrUnknown);
    assert!(IconTypeSupport::UnspecifiedOrUnknown < IconTypeSupport::Unsupported);
    assert_eq!(
        icon_type_support(Some("image/x-future-codec")),
        IconTypeSupport::UnspecifiedOrUnknown
    );
    assert_eq!(
        icon_type_support(Some("application/octet-stream")),
        IconTypeSupport::Unsupported
    );
}
