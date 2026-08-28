use browser_engine::image::{decode, ImageCache};
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

fn ico(entries: Vec<(u8, u8, u16, Vec<u8>)>) -> Vec<u8> {
    let count = entries.len();
    let mut out = vec![0u8; 6 + count * 16];
    out[2..4].copy_from_slice(&1u16.to_le_bytes());
    out[4..6].copy_from_slice(&(count as u16).to_le_bytes());
    let mut offset = out.len();
    for (i, (width, height, depth, payload)) in entries.iter().enumerate() {
        let base = 6 + i * 16;
        out[base] = *width;
        out[base + 1] = *height;
        out[base + 4..base + 6].copy_from_slice(&1u16.to_le_bytes());
        out[base + 6..base + 8].copy_from_slice(&depth.to_le_bytes());
        out[base + 8..base + 12].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        out[base + 12..base + 16].copy_from_slice(&(offset as u32).to_le_bytes());
        offset += payload.len();
    }
    for (_, _, _, payload) in entries {
        out.extend_from_slice(&payload);
    }
    out
}

#[test]
fn public_decoder_handles_png_backed_favicon() {
    let bytes = ico(vec![(1, 1, 32, png_rgba(1, 1, [12, 34, 56, 78]))]);
    let image = decode(&bytes).expect("PNG-backed ICO");
    assert_eq!((image.width, image.height), (1, 1));
    assert_eq!(image.pixel(0, 0), [12, 34, 56, 78]);
}

#[test]
fn public_decoder_selects_largest_highest_depth_icon() {
    let bytes = ico(vec![
        (1, 1, 32, png_rgba(1, 1, [255, 0, 0, 255])),
        (2, 2, 8, png_rgba(2, 2, [0, 255, 0, 255])),
        (2, 2, 32, png_rgba(2, 2, [0, 0, 255, 255])),
    ]);
    let image = decode(&bytes).expect("multi-entry ICO");
    assert_eq!((image.width, image.height), (2, 2));
    assert_eq!(image.pixel(0, 0), [0, 0, 255, 255]);
}

#[test]
fn image_cache_fetches_png_backed_favicon() {
    let bytes = ico(vec![(1, 1, 32, png_rgba(1, 1, [7, 8, 9, 200]))]);
    let mut loader = MemoryLoader::new();
    loader.insert("demo:///favicon.ico", bytes);
    let url = Url::parse("demo:///favicon.ico").unwrap();
    let mut cache = ImageCache::new();
    let image = cache.fetch(&url, &loader).expect("cached ICO");
    assert_eq!(image.pixel(0, 0), [7, 8, 9, 200]);
    assert_eq!(cache.len(), 1);
}

#[test]
fn rejects_malformed_ico_directory_and_payload() {
    let mut truncated = vec![0, 0, 1, 0, 1, 0];
    truncated.resize(10, 0);
    assert!(decode(&truncated).is_err());

    let mut bad_offset = ico(vec![(1, 1, 32, png_rgba(1, 1, [1, 2, 3, 4]))]);
    bad_offset[18..22].copy_from_slice(&1u32.to_le_bytes());
    assert!(decode(&bad_offset).is_err());

    let mismatch = ico(vec![(2, 2, 32, png_rgba(1, 1, [1, 2, 3, 4]))]);
    assert!(decode(&mismatch).is_err());
}
