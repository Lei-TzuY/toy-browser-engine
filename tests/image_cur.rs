use browser_engine::image::{decode, decode_cursor, CursorCache, ImageCache};
use browser_engine::{MemoryLoader, Url};

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

fn png_rgb(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        let mut pixels = Vec::with_capacity((width * height * 3) as usize);
        for _ in 0..width * height {
            pixels.extend_from_slice(&rgb);
        }
        writer.write_image_data(&pixels).unwrap();
    }
    out
}

fn os2_core24(rgb: [u8; 3]) -> Vec<u8> {
    let mut out = vec![0u8; 12 + 4 + 4];
    out[0..4].copy_from_slice(&12u32.to_le_bytes());
    out[4..6].copy_from_slice(&1u16.to_le_bytes());
    out[6..8].copy_from_slice(&2u16.to_le_bytes());
    out[8..10].copy_from_slice(&1u16.to_le_bytes());
    out[10..12].copy_from_slice(&24u16.to_le_bytes());
    out[12..15].copy_from_slice(&[rgb[2], rgb[1], rgb[0]]);
    out
}

fn cur(entries: Vec<(u8, u8, u16, u16, Vec<u8>)>) -> Vec<u8> {
    let count = entries.len();
    let mut out = vec![0u8; 6 + count * 16];
    out[2..4].copy_from_slice(&2u16.to_le_bytes());
    out[4..6].copy_from_slice(&(count as u16).to_le_bytes());
    let mut offset = out.len();
    for (index, (width, height, hot_x, hot_y, payload)) in entries.iter().enumerate() {
        let base = 6 + index * 16;
        out[base] = *width;
        out[base + 1] = *height;
        out[base + 4..base + 6].copy_from_slice(&hot_x.to_le_bytes());
        out[base + 6..base + 8].copy_from_slice(&hot_y.to_le_bytes());
        out[base + 8..base + 12].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        out[base + 12..base + 16].copy_from_slice(&(offset as u32).to_le_bytes());
        offset += payload.len();
    }
    for (_, _, _, _, payload) in entries {
        out.extend_from_slice(&payload);
    }
    out
}

#[test]
fn public_cursor_decoder_preserves_png_hotspot() {
    let bytes = cur(vec![(2, 2, 1, 1, png_rgba(2, 2, [12, 34, 56, 78]))]);
    let cursor = decode_cursor(&bytes).expect("PNG-backed CUR");
    assert_eq!(cursor.hotspot(), (1, 1));
    assert_eq!((cursor.image.width, cursor.image.height), (2, 2));
    assert_eq!(cursor.image.pixel(0, 0), [12, 34, 56, 78]);

    let raster = decode(&bytes).expect("CUR through generic decoder");
    assert_eq!(raster.pixel(1, 1), [12, 34, 56, 78]);
}

#[test]
fn cursor_selection_prefers_larger_then_deeper_payload() {
    let bytes = cur(vec![
        (1, 1, 0, 0, png_rgba(1, 1, [255, 0, 0, 255])),
        (2, 2, 0, 1, png_rgb(2, 2, [0, 255, 0])),
        (2, 2, 1, 0, png_rgba(2, 2, [0, 0, 255, 200])),
    ]);
    let cursor = decode_cursor(&bytes).expect("multi-entry CUR");
    assert_eq!(cursor.hotspot(), (1, 0));
    assert_eq!((cursor.image.width, cursor.image.height), (2, 2));
    assert_eq!(cursor.image.pixel(0, 0), [0, 0, 255, 200]);
}

#[test]
fn cursor_adapter_reuses_os2_core_ico_decoder() {
    let bytes = cur(vec![(1, 1, 0, 0, os2_core24([90, 80, 70]))]);
    let cursor = decode_cursor(&bytes).expect("OS/2 core CUR");
    assert_eq!(cursor.hotspot(), (0, 0));
    assert_eq!(cursor.image.pixel(0, 0), [90, 80, 70, 255]);
}

#[test]
fn cursor_and_image_caches_cover_both_metadata_and_raster_paths() {
    let bytes = cur(vec![(2, 2, 1, 1, png_rgba(2, 2, [5, 6, 7, 8]))]);
    let mut loader = MemoryLoader::new();
    loader.insert("demo:///pointer.cur", bytes);
    let url = Url::parse("demo:///pointer.cur").unwrap();

    let mut cursors = CursorCache::new();
    let cursor = cursors.fetch(&url, &loader).expect("cached cursor");
    assert_eq!(cursor.hotspot(), (1, 1));
    assert_eq!(cursor.image.pixel(0, 0), [5, 6, 7, 8]);
    assert_eq!(cursors.len(), 1);

    let mut images = ImageCache::new();
    let image = images.fetch(&url, &loader).expect("cached CUR raster");
    assert_eq!(image.pixel(0, 0), [5, 6, 7, 8]);
    assert_eq!(images.len(), 1);
}

#[test]
fn rejects_out_of_bounds_hotspots_and_truncated_directories() {
    let bad_hotspot = cur(vec![(2, 2, 2, 0, png_rgba(2, 2, [0, 0, 0, 255]))]);
    assert!(decode_cursor(&bad_hotspot).is_err());

    let mut truncated = vec![0u8; 10];
    truncated[2..4].copy_from_slice(&2u16.to_le_bytes());
    truncated[4..6].copy_from_slice(&1u16.to_le_bytes());
    assert!(decode_cursor(&truncated).is_err());
}
