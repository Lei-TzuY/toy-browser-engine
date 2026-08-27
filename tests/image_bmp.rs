use browser_engine::image::{decode, ImageCache};
use browser_engine::{MemoryLoader, Url};

fn bmp(width: i32, height: i32, bpp: u16, rows: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; 54];
    out[0..2].copy_from_slice(b"BM");
    let size = 54u32 + rows.len() as u32;
    out[2..6].copy_from_slice(&size.to_le_bytes());
    out[10..14].copy_from_slice(&54u32.to_le_bytes());
    out[14..18].copy_from_slice(&40u32.to_le_bytes());
    out[18..22].copy_from_slice(&width.to_le_bytes());
    out[22..26].copy_from_slice(&height.to_le_bytes());
    out[26..28].copy_from_slice(&1u16.to_le_bytes());
    out[28..30].copy_from_slice(&bpp.to_le_bytes());
    out.extend_from_slice(rows);
    out
}

#[test]
fn public_decoder_handles_24_bit_bgr_padding_and_bottom_up_rows() {
    let bytes = bmp(
        2,
        2,
        24,
        &[
            255, 0, 0, 255, 255, 255, 0, 0, // bottom: blue, white + padding
            0, 0, 255, 0, 255, 0, 0, 0,     // top: red, green + padding
        ],
    );
    let image = decode(&bytes).expect("24-bit BMP");
    assert_eq!((image.width, image.height), (2, 2));
    assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
    assert_eq!(image.pixel(1, 0), [0, 255, 0, 255]);
    assert_eq!(image.pixel(0, 1), [0, 0, 255, 255]);
    assert_eq!(image.pixel(1, 1), [255, 255, 255, 255]);
}

#[test]
fn image_cache_uses_bmp_decoder() {
    let bytes = bmp(1, -1, 32, &[3, 2, 1, 77]);
    let mut loader = MemoryLoader::new();
    loader.insert("demo:///pixel.bmp", bytes);
    let url = Url::parse("demo:///pixel.bmp").unwrap();
    let mut cache = ImageCache::new();

    let image = cache.fetch(&url, &loader).expect("cached BMP");
    assert_eq!(image.pixel(0, 0), [1, 2, 3, 255]);
    assert_eq!(cache.len(), 1);
}

#[test]
fn rejects_unsupported_and_malformed_bmp_layouts() {
    let mut compressed = bmp(1, 1, 24, &[0, 0, 0, 0]);
    compressed[30..34].copy_from_slice(&1u32.to_le_bytes());
    assert!(decode(&compressed).is_err());

    let unsupported_depth = bmp(1, 1, 16, &[0, 0, 0, 0]);
    assert!(decode(&unsupported_depth).is_err());

    assert!(decode(&bmp(1, 2, 24, &[0, 0, 0, 0])).is_err());
}
