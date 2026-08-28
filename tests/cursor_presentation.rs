use std::rc::Rc;

use browser_engine::cursor::CursorIcon;
use browser_engine::cursor_assets::ResolvedCursor;
use browser_engine::cursor_presentation::{
    presentation_for_cursor, presentation_for_icon, CursorPresentation, NativeCursor,
};
use browser_engine::image::{CursorImage, RasterImage};
use browser_engine::Url;

#[test]
fn public_presentation_maps_common_css_keywords() {
    assert_eq!(
        presentation_for_icon(CursorIcon::Text),
        CursorPresentation::Native(NativeCursor::IBeam)
    );
    assert_eq!(
        presentation_for_icon(CursorIcon::Pointer),
        CursorPresentation::Native(NativeCursor::OpenHand)
    );
    assert_eq!(
        presentation_for_icon(CursorIcon::Crosshair),
        CursorPresentation::Native(NativeCursor::Crosshair)
    );
    assert_eq!(
        presentation_for_icon(CursorIcon::EwResize),
        CursorPresentation::Native(NativeCursor::ResizeLeftRight)
    );
    assert_eq!(presentation_for_icon(CursorIcon::None), CursorPresentation::Hidden);
}

#[test]
fn custom_cursor_assets_choose_software_presentation() {
    let resolved = ResolvedCursor::Image {
        cursor: Rc::new(CursorImage {
            image: RasterImage::new(1, 1, vec![1, 2, 3, 255]),
            hotspot_x: 0,
            hotspot_y: 0,
        }),
        source: Url::parse("demo:///pointer.png").unwrap(),
        fallback: CursorIcon::Default,
    };

    let presentation = presentation_for_cursor(&resolved);
    assert_eq!(presentation, CursorPresentation::SoftwareImage);
    assert!(!presentation.native_visible());
    assert!(presentation.needs_software_overlay());
}
