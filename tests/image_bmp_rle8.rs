use browser_engine::image::{decode, ImageCache};
use browser_engine::{MemoryLoader, Url};

fn rle8_bmp(width: i32, height: i32, palette: &[[u8; 4]], stream: &[u8]) -> Vec<u8> {
    let pixel_offset = 54 + palette.len() * 4;
    let mut out = vec![0u8; 54];
    out[0..2].copy_from_slice(b"BM");
    out[2..6].copy_from_slice(&((pixel_offset + stream.len()) as u32).to_le_bytes());
    out[10..14].copy_from_slice(&(pixel_offset as u32).to_le_bytes());
    out[14..18].copy_from_slice(&40u32.to_le_bytes());
    out[18..22].copy_from_slice(&width.to_le_bytes());
    out[22..26].copy_from_slice(&height.to_le_bytes());
    out[26..28].copy_from_slice(&1u16.to_le_bytes());
    out[28..30].copy_from_slice(&8u16.to_le_bytes());
    out[30..34].copy_from_slice(&1u32.to_le_bytes());
    out[34..38].copy_from_slice(&(stream.len() as u32).to_le_bytes());
    out[46..50].copy_from_slice(&(palette.len() as u32).to_le_bytes());
    for entry in palette {
        out.extend_from_slice(entry);
    }
    out.extend_from_slice(stream);
    out
}

#[test]
fn public_decoder_handles_encoded_absolute_and_bottom_up_rle8() {
    let palette = [
        [0, 0, 0, 0],
        [0, 0, 255, 0],
        [0, 255, 0, 0],
        [255, 0, 0, 0],
    ];
    let bytes = rle8_bmp(
        4,
        2,
        &palette,
        &[
            4, 3, 0, 0,
            0, 4, 1, 2, 1, 2, 0, 0,
            0, 1,
        ],
    );
    let image = decode(&bytes).expect("BI_RLE8 BMP");
    assert_eq!((image.width, image.height), (4, 2));
    assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
    assert_eq!(image.pixel(1, 0), [0, 255, 0, 255]);
    assert_eq!(image.pixel(0, 1), [0, 0, 255, 255]);
    assert_eq!(image.pixel(3, 1), [0, 0, 255, 255]);
}

#[test]
fn image_cache_uses_rle8_decoder_and_delta_background() {
    let palette = [[10, 20, 30, 0], [0, 0, 255, 0]];
    let bytes = rle8_bmp(3, 1, &palette, &[0, 2, 1, 0, 1, 1, 0, 1]);
    let mut loader = MemoryLoader::new();
    loader.insert("demo:///compressed.bmp", bytes);
    let url = Url::parse("demo:///compressed.bmp").unwrap();
    let mut cache = ImageCache::new();

    let image = cache.fetch(&url, &loader).expect("cached RLE8 BMP");
    assert_eq!(image.pixel(0, 0), [30, 20, 10, 255]);
    assert_eq!(image.pixel(1, 0), [255, 0, 0, 255]);
    assert_eq!(image.pixel(2, 0), [30, 20, 10, 255]);
    assert_eq!(cache.len(), 1);
}

#[test]
fn rejects_malformed_rle8_streams() {
    let palette = [[0, 0, 0, 0], [0, 0, 255, 0]];
    assert!(decode(&rle8_bmp(2, 1, &palette, &[3, 1, 0, 1])).is_err());
    assert!(decode(&rle8_bmp(2, 1, &palette, &[0, 3, 1, 1])).is_err());
    assert!(decode(&rle8_bmp(2, 1, &palette, &[2, 1])).is_err());
    assert!(decode(&rle8_bmp(2, -1, &palette, &[0, 1])).is_err());
}
