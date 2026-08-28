// ============================================================
// cursor_overlay.rs — software-rendered custom cursor images
// ============================================================

use crate::cursor_assets::ResolvedCursor;
use crate::image::CursorImage;
use crate::paint::Canvas;

/// Statistics for one custom-cursor compositing pass.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CursorOverlayStats {
    /// Source pixels whose destination coordinate was inside the canvas.
    pub sampled_pixels: usize,
    /// Non-transparent source pixels that changed or replaced a canvas pixel.
    pub blended_pixels: usize,
    /// Source pixels outside the canvas after hotspot positioning.
    pub clipped_pixels: usize,
}

/// Alpha-composite a decoded cursor over the final opaque RGB page canvas.
///
/// `pointer_x`/`pointer_y` are viewport/canvas coordinates. The decoded CUR
/// hotspot is subtracted before drawing, matching native cursor positioning.
/// Pixels outside the viewport are clipped rather than shifting the image.
pub fn paint_cursor_image(
    canvas: &mut Canvas,
    cursor: &CursorImage,
    pointer_x: f32,
    pointer_y: f32,
) -> CursorOverlayStats {
    let mut stats = CursorOverlayStats::default();
    if !pointer_x.is_finite() || !pointer_y.is_finite() {
        return stats;
    }

    let origin_x = pointer_x.floor() as i64 - i64::from(cursor.hotspot_x);
    let origin_y = pointer_y.floor() as i64 - i64::from(cursor.hotspot_y);
    let canvas_width = i64::try_from(canvas.width).unwrap_or(i64::MAX);
    let canvas_height = i64::try_from(canvas.height).unwrap_or(i64::MAX);
    let image_width = cursor.image.width as usize;
    let image_height = cursor.image.height as usize;

    for source_y in 0..image_height {
        let dest_y = origin_y + source_y as i64;
        for source_x in 0..image_width {
            let dest_x = origin_x + source_x as i64;
            if dest_x < 0 || dest_y < 0 || dest_x >= canvas_width || dest_y >= canvas_height {
                stats.clipped_pixels += 1;
                continue;
            }
            stats.sampled_pixels += 1;

            let source_index = (source_y * image_width + source_x) * 4;
            let Some(source) = cursor.image.pixels.get(source_index..source_index + 4) else {
                // `RasterImage::new` guarantees this shape. Keep this routine
                // defensive for manually constructed public RasterImage values.
                continue;
            };
            let alpha = source[3];
            if alpha == 0 {
                continue;
            }

            let dest_x = dest_x as usize;
            let dest_y = dest_y as usize;
            let dest_index = (dest_y * canvas.width + dest_x) * 3;
            let destination = &mut canvas.pixels[dest_index..dest_index + 3];

            if alpha == 255 {
                destination.copy_from_slice(&source[..3]);
            } else {
                destination[0] = blend_channel(source[0], destination[0], alpha);
                destination[1] = blend_channel(source[1], destination[1], alpha);
                destination[2] = blend_channel(source[2], destination[2], alpha);
            }
            stats.blended_pixels += 1;
        }
    }

    stats
}

/// Paint a custom image cursor when `resolved` contains one. System cursors are
/// deliberately left to the native window backend and return `None`.
pub fn paint_resolved_cursor(
    canvas: &mut Canvas,
    resolved: &ResolvedCursor,
    pointer_x: f32,
    pointer_y: f32,
) -> Option<CursorOverlayStats> {
    match resolved {
        ResolvedCursor::Image { cursor, .. } => {
            Some(paint_cursor_image(canvas, cursor, pointer_x, pointer_y))
        }
        ResolvedCursor::System(_) => None,
    }
}

#[inline]
fn blend_channel(source: u8, destination: u8, alpha: u8) -> u8 {
    let alpha = alpha as u32;
    let inverse = 255 - alpha;
    ((source as u32 * alpha + destination as u32 * inverse + 127) / 255) as u8
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;
    use crate::css::parser::Color;
    use crate::cursor::CursorIcon;
    use crate::cursor_assets::ResolvedCursor;
    use crate::image::{CursorImage, RasterImage};
    use crate::net::Url;

    fn cursor(width: u32, height: u32, hotspot: (u16, u16), pixels: Vec<u8>) -> CursorImage {
        CursorImage {
            image: RasterImage::new(width, height, pixels),
            hotspot_x: hotspot.0,
            hotspot_y: hotspot.1,
        }
    }

    #[test]
    fn hotspot_positions_opaque_cursor_pixels() {
        let cursor = cursor(
            2,
            2,
            (1, 1),
            vec![
                255, 0, 0, 255, 0, 255, 0, 255,
                0, 0, 255, 255, 255, 255, 0, 255,
            ],
        );
        let mut canvas = Canvas::new(3, 3, Color::rgb(10, 10, 10));
        let stats = paint_cursor_image(&mut canvas, &cursor, 1.0, 1.0);

        assert_eq!(stats.sampled_pixels, 4);
        assert_eq!(stats.blended_pixels, 4);
        assert_eq!(stats.clipped_pixels, 0);
        assert_eq!(&canvas.pixels[0..3], &[255, 0, 0]);
        assert_eq!(&canvas.pixels[3..6], &[0, 255, 0]);
        let second_row = canvas.width * 3;
        assert_eq!(&canvas.pixels[second_row..second_row + 3], &[0, 0, 255]);
    }

    #[test]
    fn clips_at_viewport_edges_without_moving_hotspot() {
        let cursor = cursor(
            2,
            2,
            (1, 1),
            vec![
                255, 0, 0, 255, 0, 255, 0, 255,
                0, 0, 255, 255, 20, 30, 40, 255,
            ],
        );
        let mut canvas = Canvas::new(2, 2, Color::rgb(1, 1, 1));
        let stats = paint_cursor_image(&mut canvas, &cursor, 0.0, 0.0);

        assert_eq!(stats.clipped_pixels, 3);
        assert_eq!(stats.sampled_pixels, 1);
        assert_eq!(stats.blended_pixels, 1);
        assert_eq!(&canvas.pixels[0..3], &[20, 30, 40]);
    }

    #[test]
    fn blends_straight_rgba_over_opaque_canvas() {
        let cursor = cursor(1, 1, (0, 0), vec![200, 0, 0, 128]);
        let mut canvas = Canvas::new(1, 1, Color::rgb(100, 100, 100));
        let stats = paint_cursor_image(&mut canvas, &cursor, 0.0, 0.0);

        assert_eq!(stats.blended_pixels, 1);
        assert_eq!(canvas.pixels, vec![150, 50, 50]);
    }

    #[test]
    fn transparent_and_non_finite_inputs_are_noops() {
        let transparent = cursor(1, 1, (0, 0), vec![255, 0, 0, 0]);
        let mut canvas = Canvas::new(1, 1, Color::rgb(7, 8, 9));
        assert_eq!(
            paint_cursor_image(&mut canvas, &transparent, 0.0, 0.0).blended_pixels,
            0
        );
        assert_eq!(canvas.pixels, vec![7, 8, 9]);

        assert_eq!(
            paint_cursor_image(&mut canvas, &transparent, f32::NAN, 0.0),
            CursorOverlayStats::default()
        );
    }

    #[test]
    fn resolved_system_cursor_is_not_software_painted() {
        let mut canvas = Canvas::new(1, 1, Color::rgb(1, 2, 3));
        let system = ResolvedCursor::System(CursorIcon::Pointer);
        assert_eq!(paint_resolved_cursor(&mut canvas, &system, 0.0, 0.0), None);

        let custom = ResolvedCursor::Image {
            cursor: Rc::new(cursor(1, 1, (0, 0), vec![9, 8, 7, 255])),
            source: Url::parse("demo:///cursor.cur").unwrap(),
            fallback: CursorIcon::Default,
        };
        assert_eq!(
            paint_resolved_cursor(&mut canvas, &custom, 0.0, 0.0)
                .unwrap()
                .blended_pixels,
            1
        );
        assert_eq!(canvas.pixels, vec![9, 8, 7]);
    }
}
