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

fn dib24(width: u32, height: u32, pixels_top_down: &[[u8; 3]], transparent: &[(u32, u32)]) -> Vec<u8> {
    assert_eq!(pixels_top_down.len(), (width * height) as usize);
    let xor_stride = ((width as usize * 24 + 31) / 32) * 4;
    let and_stride = ((width as usize + 31) / 32) * 4;
    let mut out = vec![0u8; 40 + xor_stride * height as usize + and_stride * height as usize];
    out[0..4].copy_from_slice(&40u32.to_le_bytes());
    out[4..8].copy_from_slice(&(width as i32).to_le_bytes());
    out[8..12].copy_from_slice(&((height as i32) * 2).to_le_bytes());
    out[12..14].copy_from_slice(&1u16.to_le_bytes());
    out[14..16].copy_from_slice(&24u16.to_le_bytes());
    for source_row in 0..height as usize {
        let image_y = height as usize - 1 - source_row;
        let row = 40 + source_row * xor_stride;
        for x in 0..width as usize {
            let rgb = pixels_top_down[image_y * width as usize + x];
            let base = row + x * 3;
            out[base..base + 3].copy_from_slice(&[rgb[2], rgb[1], rgb[0]]);
        }
    }
    let and_start = 40 + xor_stride * height as usize;
    for &(x, y) in transparent {
        let source_y = height as usize - 1 - y as usize;
        let byte = and_start + source_y * and_stride + x as usize / 8;
        out[byte] |= 1 << (7 - (x as usize % 8));
    }
    out
}

fn dib_indexed(bit_depth: u16, indices: &[u8], palette: &[[u8; 3]]) -> Vec<u8> {
    assert!(matches!(bit_depth, 1 | 4 | 8));
    let width = indices.len();
    let maximum = 1usize << bit_depth as usize;
    assert!(!palette.is_empty() && palette.len() <= maximum);
    assert!(indices.iter().all(|index| (*index as usize) < palette.len()));
    let xor_stride = ((width * bit_depth as usize + 31) / 32) * 4;
    let and_stride = ((width + 31) / 32) * 4;
    let palette_bytes = palette.len() * 4;
    let xor_start = 40 + palette_bytes;
    let mut out = vec![0u8; xor_start + xor_stride + and_stride];
    out[0..4].copy_from_slice(&40u32.to_le_bytes());
    out[4..8].copy_from_slice(&(width as i32).to_le_bytes());
    out[8..12].copy_from_slice(&2i32.to_le_bytes());
    out[12..14].copy_from_slice(&1u16.to_le_bytes());
    out[14..16].copy_from_slice(&bit_depth.to_le_bytes());
    out[32..36].copy_from_slice(&(palette.len() as u32).to_le_bytes());
    for (index, rgb) in palette.iter().enumerate() {
        let base = 40 + index * 4;
        out[base..base + 4].copy_from_slice(&[rgb[2], rgb[1], rgb[0], 0]);
    }
    match bit_depth {
        1 => {
            for (x, index) in indices.iter().copied().enumerate() {
                out[xor_start + x / 8] |= (index & 1) << (7 - (x % 8));
            }
        }
        4 => {
            for (x, index) in indices.iter().copied().enumerate() {
                let byte = &mut out[xor_start + x / 2];
                if x % 2 == 0 {
                    *byte |= (index & 0x0f) << 4;
                } else {
                    *byte |= index & 0x0f;
                }
            }
        }
        8 => out[xor_start..xor_start + width].copy_from_slice(indices),
        _ => unreachable!(),
    }
    out
}

fn dib16(value: u16) -> Vec<u8> {
    let mut out = vec![0u8; 40 + 4 + 4];
    out[0..4].copy_from_slice(&40u32.to_le_bytes());
    out[4..8].copy_from_slice(&1i32.to_le_bytes());
    out[8..12].copy_from_slice(&2i32.to_le_bytes());
    out[12..14].copy_from_slice(&1u16.to_le_bytes());
    out[14..16].copy_from_slice(&16u16.to_le_bytes());
    out[40..42].copy_from_slice(&value.to_le_bytes());
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
fn public_decoder_handles_dib_backed_favicon_and_and_mask() {
    let bytes = ico(vec![(
        2,
        2,
        24,
        dib24(
            2,
            2,
            &[
                [255, 0, 0],
                [0, 255, 0],
                [0, 0, 255],
                [255, 255, 0],
            ],
            &[(1, 0)],
        ),
    )]);
    let image = decode(&bytes).expect("DIB-backed ICO");
    assert_eq!((image.width, image.height), (2, 2));
    assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
    assert_eq!(image.pixel(1, 0), [0, 255, 0, 0]);
    assert_eq!(image.pixel(0, 1), [0, 0, 255, 255]);
    assert_eq!(image.pixel(1, 1), [255, 255, 0, 255]);
}

#[test]
fn public_decoder_handles_indexed_dib_depths() {
    let one = decode(&ico(vec![(
        2,
        1,
        1,
        dib_indexed(1, &[0, 1], &[[255, 0, 0], [0, 255, 0]]),
    )]))
    .expect("1-bit DIB ICO");
    assert_eq!(one.pixel(0, 0), [255, 0, 0, 255]);
    assert_eq!(one.pixel(1, 0), [0, 255, 0, 255]);

    let four = decode(&ico(vec![(
        2,
        1,
        4,
        dib_indexed(4, &[1, 2], &[[0, 0, 0], [0, 0, 255], [255, 255, 0]]),
    )]))
    .expect("4-bit DIB ICO");
    assert_eq!(four.pixel(0, 0), [0, 0, 255, 255]);
    assert_eq!(four.pixel(1, 0), [255, 255, 0, 255]);

    let eight = decode(&ico(vec![(
        2,
        1,
        8,
        dib_indexed(8, &[1, 2], &[[0, 0, 0], [255, 0, 255], [0, 255, 255]]),
    )]))
    .expect("8-bit DIB ICO");
    assert_eq!(eight.pixel(0, 0), [255, 0, 255, 255]);
    assert_eq!(eight.pixel(1, 0), [0, 255, 255, 255]);
}

#[test]
fn public_decoder_handles_16bit_rgb555_dib() {
    let red = decode(&ico(vec![(1, 1, 16, dib16(0x7c00))])).expect("16-bit DIB ICO");
    assert_eq!(red.pixel(0, 0), [255, 0, 0, 255]);
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
fn image_cache_fetches_dib_backed_favicon() {
    let bytes = ico(vec![(1, 1, 24, dib24(1, 1, &[[7, 8, 9]], &[]))]);
    let mut loader = MemoryLoader::new();
    loader.insert("demo:///legacy.ico", bytes);
    let url = Url::parse("demo:///legacy.ico").unwrap();
    let mut cache = ImageCache::new();
    let image = cache.fetch(&url, &loader).expect("cached DIB ICO");
    assert_eq!(image.pixel(0, 0), [7, 8, 9, 255]);
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
