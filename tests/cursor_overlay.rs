use browser_engine::css::parser::Color;
use browser_engine::cursor_overlay::{paint_cursor_image, CursorOverlayStats};
use browser_engine::image::{CursorImage, RasterImage};
use browser_engine::paint::Canvas;

#[test]
fn public_overlay_uses_cursor_hotspot_and_alpha() {
    let cursor = CursorImage {
        image: RasterImage::new(
            2,
            1,
            vec![
                255, 0, 0, 255,
                0, 0, 200, 128,
            ],
        ),
        hotspot_x: 1,
        hotspot_y: 0,
    };
    let mut canvas = Canvas::new(3, 1, Color::rgb(100, 100, 100));

    let stats = paint_cursor_image(&mut canvas, &cursor, 1.0, 0.0);
    assert_eq!(
        stats,
        CursorOverlayStats {
            sampled_pixels: 2,
            blended_pixels: 2,
            clipped_pixels: 0,
        }
    );
    assert_eq!(&canvas.pixels[0..3], &[255, 0, 0]);
    assert_eq!(&canvas.pixels[3..6], &[50, 50, 150]);
}

#[test]
fn public_overlay_clips_negative_hotspot_origin() {
    let cursor = CursorImage {
        image: RasterImage::new(
            2,
            2,
            vec![
                1, 2, 3, 255, 4, 5, 6, 255,
                7, 8, 9, 255, 10, 11, 12, 255,
            ],
        ),
        hotspot_x: 1,
        hotspot_y: 1,
    };
    let mut canvas = Canvas::new(1, 1, Color::rgb(0, 0, 0));

    let stats = paint_cursor_image(&mut canvas, &cursor, 0.0, 0.0);
    assert_eq!(stats.sampled_pixels, 1);
    assert_eq!(stats.clipped_pixels, 3);
    assert_eq!(canvas.pixels, vec![10, 11, 12]);
}
