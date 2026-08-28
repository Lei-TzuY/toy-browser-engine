use std::rc::Rc;

use browser_engine::css::parser::Color;
use browser_engine::cursor::CursorIcon;
use browser_engine::cursor_assets::{CursorResolver, ResolvedCursor};
use browser_engine::cursor_frame::{compose_cursor_frame, prepare_cursor_frame};
use browser_engine::cursor_presentation::{CursorPresentation, NativeCursor};
use browser_engine::image::{CursorImage, RasterImage};
use browser_engine::paint::Canvas;
use browser_engine::script::dom_api;
use browser_engine::{Browser, MemoryLoader, PointerState, Url};

#[test]
fn frame_helper_resolves_computed_cursor_for_hit_target() {
    let mut loader = MemoryLoader::new();
    loader.insert(
        "demo:///index.html",
        b"<style>#target { cursor: pointer; }</style><div id='target'>target</div>".to_vec(),
    );
    let browser = Browser::open(
        Box::new(loader),
        &Url::parse("demo:///index.html").unwrap(),
    )
    .unwrap();
    let path = dom_api::query_selector(&browser.document().dom, &[], "#target").unwrap();
    let pointer = PointerState {
        hovered: Some(path.clone()),
        focused: Some(path.clone()),
        active: None,
    };
    let mut resolver = CursorResolver::new();
    let mut canvas = Canvas::new(8, 8, Color::rgb(1, 2, 3));

    let outcome = prepare_cursor_frame(
        &mut resolver,
        &browser,
        &mut canvas,
        Some(&path),
        Some((4.0, 4.0)),
        800.0,
        &pointer,
    );

    assert_eq!(
        outcome.presentation,
        CursorPresentation::Native(NativeCursor::OpenHand)
    );
    assert_eq!(outcome.overlay, None);
}

#[test]
fn public_frame_helper_composites_custom_cursor_and_requests_software_mode() {
    let resolved = ResolvedCursor::Image {
        cursor: Rc::new(CursorImage {
            image: RasterImage::new(1, 1, vec![90, 100, 110, 255]),
            hotspot_x: 0,
            hotspot_y: 0,
        }),
        source: Url::parse("demo:///pointer.png").unwrap(),
        fallback: CursorIcon::Default,
    };
    let mut canvas = Canvas::new(3, 3, Color::rgb(0, 0, 0));

    let outcome = compose_cursor_frame(&mut canvas, &resolved, Some((2.0, 1.0)));

    assert_eq!(outcome.presentation, CursorPresentation::SoftwareImage);
    assert_eq!(outcome.overlay.unwrap().blended_pixels, 1);
    let offset = (canvas.width + 2) * 3;
    assert_eq!(&canvas.pixels[offset..offset + 3], &[90, 100, 110]);
}

#[test]
fn no_pointer_uses_safe_native_default() {
    let mut loader = MemoryLoader::new();
    loader.insert("demo:///index.html", b"<p>hello</p>".to_vec());
    let browser = Browser::open(
        Box::new(loader),
        &Url::parse("demo:///index.html").unwrap(),
    )
    .unwrap();
    let mut resolver = CursorResolver::new();
    let mut canvas = Canvas::new(2, 2, Color::rgb(0, 0, 0));

    let outcome = prepare_cursor_frame(
        &mut resolver,
        &browser,
        &mut canvas,
        None,
        None,
        800.0,
        &PointerState::default(),
    );

    assert_eq!(
        outcome.presentation,
        CursorPresentation::Native(NativeCursor::Arrow)
    );
    assert_eq!(outcome.overlay, None);
}
