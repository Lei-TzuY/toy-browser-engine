use browser_engine::image::{decode, ImageCache};
use browser_engine::{MemoryLoader, Url};

fn rle_ico(
    width: u8,
    height: u8,
    bit_depth: u16,
    compression: u32,
    palette: &[[u8; 4]],
    rle: &[u8],
    and_mask: &[u8],
    size_image: Option<u32>,
) -> Vec<u8> {
    let mut dib = vec![0u8; 40];
    dib[0..4].copy_from_slice(&40u32.to_le_bytes());
    dib[4..8].copy_from_slice(&(width as i32).to_le_bytes());
    dib[8..12].copy_from_slice(&((height as i32) * 2).to_le_bytes());
    dib[12..14].copy_from_slice(&1u16.to_le_bytes());
    dib[14..16].copy_from_slice(&bit_depth.to_le_bytes());
    dib[16..20].copy_from_slice(&compression.to_le_bytes());
    dib[20..24].copy_from_slice(&size_image.unwrap_or(rle.len() as u32).to_le_bytes());
    dib[32..36].copy_from_slice(&(palette.len() as u32).to_le_bytes());
    for color in palette {
        dib.extend_from_slice(color);
    }
    dib.extend_from_slice(rle);
    dib.extend_from_slice(and_mask);

    let mut out = vec![0u8; 22];
    out[2..4].copy_from_slice(&1u16.to_le_bytes());
    out[4..6].copy_from_slice(&1u16.to_le_bytes());
    out[6] = width;
    out[7] = height;
    out[10..12].copy_from_slice(&1u16.to_le_bytes());
    out[12..14].copy_from_slice(&bit_depth.to_le_bytes());
    out[14..18].copy_from_slice(&(dib.len() as u32).to_le_bytes());
    out[18..22].copy_from_slice(&22u32.to_le_bytes());
    out.extend_from_slice(&dib);
    out
}

#[test]
fn public_decoder_handles_rle8_ico_and_binary_transparency() {
    let palette = [
        [0, 0, 0, 0],
        [0, 0, 255, 0],
        [0, 255, 0, 0],
    ];
    let rle = [2, 1, 0, 0, 2, 2, 0, 0, 0, 1];
    let and_mask = [0x40, 0, 0, 0, 0, 0, 0, 0];
    let bytes = rle_ico(2, 2, 8, 1, &palette, &rle, &and_mask, None);

    let image = decode(&bytes).expect("RLE8 ICO");
    assert_eq!(image.pixel(0, 0), [0, 255, 0, 255]);
    assert_eq!(image.pixel(1, 0), [0, 255, 0, 255]);
    assert_eq!(image.pixel(0, 1), [255, 0, 0, 255]);
    assert_eq!(image.pixel(1, 1), [255, 0, 0, 0]);
}

#[test]
fn public_decoder_handles_rle4_absolute_mode() {
    let palette = [
        [0, 0, 0, 0],
        [0, 0, 255, 0],
        [0, 255, 0, 0],
        [255, 0, 0, 0],
    ];
    let rle = [0, 4, 0x12, 0x30, 0, 1];
    let bytes = rle_ico(4, 1, 4, 2, &palette, &rle, &[0, 0, 0, 0], Some(0));

    let image = decode(&bytes).expect("RLE4 ICO");
    assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
    assert_eq!(image.pixel(1, 0), [0, 255, 0, 255]);
    assert_eq!(image.pixel(2, 0), [0, 0, 255, 255]);
    assert_eq!(image.pixel(3, 0), [0, 0, 0, 255]);
}

#[test]
fn image_cache_fetches_rle_compressed_ico() {
    let palette = [[0, 0, 0, 0], [0, 0, 255, 0]];
    let rle = [1, 1, 0, 1];
    let bytes = rle_ico(1, 1, 8, 1, &palette, &rle, &[0, 0, 0, 0], None);

    let mut loader = MemoryLoader::new();
    loader.insert("demo:///rle.ico", bytes);
    let url = Url::parse("demo:///rle.ico").unwrap();
    let mut cache = ImageCache::new();
    let image = cache.fetch(&url, &loader).expect("cached RLE ICO");

    assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
    assert_eq!(cache.len(), 1);
}

#[test]
fn rejects_rle_depth_mismatch_delta_overflow_and_truncated_mask() {
    let palette = [[0, 0, 0, 0], [0, 0, 255, 0]];

    let wrong_depth = rle_ico(1, 1, 4, 1, &palette, &[1, 1, 0, 1], &[0, 0, 0, 0], None);
    assert!(decode(&wrong_depth).is_err());

    let bad_delta = rle_ico(
        1,
        1,
        8,
        1,
        &palette,
        &[0, 2, 2, 0, 0, 1],
        &[0, 0, 0, 0],
        None,
    );
    assert!(decode(&bad_delta).is_err());

    let truncated_mask = rle_ico(1, 1, 8, 1, &palette, &[1, 1, 0, 1], &[], None);
    assert!(decode(&truncated_mask).is_err());
}
