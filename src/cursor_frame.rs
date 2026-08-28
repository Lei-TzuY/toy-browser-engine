// ============================================================
// cursor_frame.rs — per-frame cursor resolution and composition
// ============================================================

use crate::browser::Browser;
use crate::cursor::CursorIcon;
use crate::cursor_assets::{CursorResolver, ResolvedCursor};
use crate::cursor_overlay::{paint_resolved_cursor, CursorOverlayStats};
use crate::cursor_presentation::{presentation_for_cursor, CursorPresentation};
use crate::document::PointerState;
use crate::paint::Canvas;
use crate::script::NodePath;

/// Result of preparing the cursor for one frontend frame.
///
/// The backend applies `presentation` to its native window cursor. When the
/// presentation is software-backed, the image has already been composited onto
/// the supplied page canvas and `overlay` describes that paint pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorFrameOutcome {
    pub presentation: CursorPresentation,
    pub overlay: Option<CursorOverlayStats>,
}

/// Resolve the cursor for the already hit-tested target and prepare it for
/// display in the current frame.
///
/// Frontends generally already perform hit-testing to build `PointerState`.
/// Accepting that target here avoids a second layout hit-test while still
/// letting the resource resolver rebuild computed style for `:hover`, inherited
/// cursor values, author rules, and URL-backed cursor assets.
///
/// `pointer_position` is in viewport/canvas coordinates, not document
/// coordinates. That distinction is important for scrolled pages: custom cursor
/// images remain attached to the physical pointer while the page scrolls below
/// them.
pub fn prepare_cursor_frame(
    resolver: &mut CursorResolver,
    browser: &Browser,
    canvas: &mut Canvas,
    target: Option<&NodePath>,
    pointer_position: Option<(f32, f32)>,
    viewport_width: f32,
    pointer: &PointerState,
) -> CursorFrameOutcome {
    let resolved = match (target, pointer_position) {
        (Some(path), Some((x, y))) if x.is_finite() && y.is_finite() => resolver
            .resolve_for_path(browser, path, viewport_width, pointer)
            .unwrap_or(ResolvedCursor::System(CursorIcon::Default)),
        _ => ResolvedCursor::System(CursorIcon::Default),
    };

    compose_cursor_frame(canvas, &resolved, pointer_position)
}

/// Convert a resolved cursor into a backend presentation and, when necessary,
/// composite its software image over the final page canvas.
///
/// Keeping this step toolkit-neutral makes the frame behavior directly
/// regression-testable. A window backend only has to call its equivalent of
/// `apply_cursor_presentation(outcome.presentation)` after this function.
pub fn compose_cursor_frame(
    canvas: &mut Canvas,
    resolved: &ResolvedCursor,
    pointer_position: Option<(f32, f32)>,
) -> CursorFrameOutcome {
    let presentation = presentation_for_cursor(resolved);
    let overlay = match (presentation, pointer_position) {
        (CursorPresentation::SoftwareImage, Some((x, y))) if x.is_finite() && y.is_finite() => {
            paint_resolved_cursor(canvas, resolved, x, y)
        }
        _ => None,
    };

    CursorFrameOutcome {
        presentation,
        overlay,
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;
    use crate::css::parser::Color;
    use crate::cursor_presentation::NativeCursor;
    use crate::image::{CursorImage, RasterImage};
    use crate::net::Url;

    #[test]
    fn native_cursor_does_not_touch_page_pixels() {
        let mut canvas = Canvas::new(2, 2, Color::rgb(7, 8, 9));
        let before = canvas.pixels.clone();
        let resolved = ResolvedCursor::System(CursorIcon::Pointer);

        let outcome = compose_cursor_frame(&mut canvas, &resolved, Some((1.0, 1.0)));

        assert_eq!(
            outcome.presentation,
            CursorPresentation::Native(NativeCursor::OpenHand)
        );
        assert_eq!(outcome.overlay, None);
        assert_eq!(canvas.pixels, before);
    }

    #[test]
    fn software_cursor_is_composited_at_viewport_pointer() {
        let cursor = CursorImage {
            image: RasterImage::new(1, 1, vec![200, 10, 20, 255]),
            hotspot_x: 0,
            hotspot_y: 0,
        };
        let resolved = ResolvedCursor::Image {
            cursor: Rc::new(cursor),
            source: Url::parse("demo:///pointer.png").unwrap(),
            fallback: CursorIcon::Default,
        };
        let mut canvas = Canvas::new(3, 3, Color::rgb(1, 2, 3));

        let outcome = compose_cursor_frame(&mut canvas, &resolved, Some((1.0, 2.0)));

        assert_eq!(outcome.presentation, CursorPresentation::SoftwareImage);
        assert_eq!(outcome.overlay.unwrap().blended_pixels, 1);
        let pixel = (2 * canvas.width + 1) * 3;
        assert_eq!(&canvas.pixels[pixel..pixel + 3], &[200, 10, 20]);
    }

    #[test]
    fn missing_pointer_position_falls_back_without_overlay() {
        let mut canvas = Canvas::new(1, 1, Color::rgb(1, 1, 1));
        let resolved = ResolvedCursor::System(CursorIcon::None);
        let outcome = compose_cursor_frame(&mut canvas, &resolved, None);
        assert_eq!(outcome.presentation, CursorPresentation::Hidden);
        assert_eq!(outcome.overlay, None);
    }
}
