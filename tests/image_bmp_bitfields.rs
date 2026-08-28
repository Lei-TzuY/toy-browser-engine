use browser_engine::image::{decode, ImageCache};
use browser_engine::{MemoryLoader, Url};

fn bitfields_bmp(
    width: i32,
    height: i32,
    depth: u16,
    compression: u32,
    masks: &[u32],
    raster: &[u8],
) -> Vec<u8> {
    let offset = 54 + masks.len() * 4;
    let mut out = vec![0u8; offset];
    out[0..2].copy_from_slice(b"BM");
    out[2..6].copy_from_slice(&((offset + raster.len()) as u32).to_le_bytes());
    out[10..14].copy_from_slice(&(offset as u32).to_le_bytes());
    out[14..18].copy_from_slice(&40u32.to_le_bytes());
    out[18..22].copy_from_slice(&width.to_le_bytes());
    out[22..26].copy_from_slice(&height.to_le_bytes());
    out[26..28].copy_from_slice(&1u16.to_le_bytes());
    out[28..30].copy_from_slice(&depth.to_le_bytes());
    out[30..34].copy_from_slice(&compression.to_le_bytes());
    for (i, mask) in masks.iter().copied().enumerate() {
        out[54 + i * 4..58 + i * 4].copy_from_slice(&mask.to_le_bytes());
    }
    out.extend_from_slice(raster);
    out
}

#[test]
fn public_decoder_handles_rgb565_scaling_and_padding() {
    let bytes = bitfields_bmp(
        3,
        1,
        16,
        3,
        &[0xf800, 0x07e0, 0x001f],
        &[0x00, 0xf8, 0xe0, 0x07, 0x1f, 0x00, 0xaa, 0xbb],
    );
    let image = decode(&bytes).expect("RGB565 bitfields BMP");
    assert_eq!((image.width, image.height), (3, 1));
    assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
    assert_eq!(image.pixel(1, 0), [0, 255, 0, 255]);
    assert_eq!(image.pixel(2, 0), [0, 0, 255, 255]);
}

#[test]
fn image_cache_handles_top_down_32_bit_bitfields() {
    let bytes = bitfields_bmp(
        1,
        -2,
        32,
        3,
        &[0x00ff0000, 0x0000ff00, 0x000000ff],
        &[0x33, 0x22, 0x11, 0x99, 0x66, 0x55, 0x44, 0x88],
    );
    let mut loader = MemoryLoader::new();
    loader.insert("demo:///bitfields.bmp", bytes);
    let url = Url::parse("demo:///bitfields.bmp").unwrap();
    let mut cache = ImageCache::new();

    let image = cache.fetch(&url, &loader).expect("cached bitfields BMP");
    assert_eq!(image.pixel(0, 0), [0x11, 0x22, 0x33, 255]);
    assert_eq!(image.pixel(0, 1), [0x44, 0x55, 0x66, 255]);
    assert_eq!(cache.len(), 1);
}

#[test]
fn public_decoder_preserves_alpha_bitfields() {
    let bytes = bitfields_bmp(
        2,
        1,
        32,
        6,
        &[0x00ff0000, 0x0000ff00, 0x000000ff, 0xff000000],
        &[0x30, 0x20, 0x10, 0x00, 0x60, 0x50, 0x40, 0x80],
    );
    let image = decode(&bytes).expect("BI_ALPHABITFIELDS BMP");
    assert_eq!(image.pixel(0, 0), [0x10, 0x20, 0x30, 0x00]);
    assert_eq!(image.pixel(1, 0), [0x40, 0x50, 0x60, 0x80]);
}

#[test]
fn image_cache_preserves_scaled_alpha_masks() {
    let bytes = bitfields_bmp(
        1,
        -1,
        32,
        6,
        &[0x000000ff, 0x0000ff00, 0x00ff0000, 0x03000000],
        &[0xff, 0x80, 0x40, 0x02],
    );
    let mut loader = MemoryLoader::new();
    loader.insert("demo:///alpha-bitfields.bmp", bytes);
    let url = Url::parse("demo:///alpha-bitfields.bmp").unwrap();
    let mut cache = ImageCache::new();

    let image = cache.fetch(&url, &loader).expect("cached alpha bitfields BMP");
    assert_eq!(image.pixel(0, 0), [255, 128, 64, 170]);
}

#[test]
fn rejects_invalid_masks_and_truncated_raster() {
    assert!(decode(&bitfields_bmp(1, 1, 16, 3, &[0xf800, 0x7800, 0x001f], &[0, 0, 0, 0])).is_err());
    assert!(decode(&bitfields_bmp(1, 1, 16, 3, &[0xa800, 0x0700, 0x001f], &[0, 0, 0, 0])).is_err());
    assert!(decode(&bitfields_bmp(1, 1, 16, 3, &[0xf800, 0x07e0, 0x001f], &[0, 0])).is_err());

    assert!(decode(&bitfields_bmp(
        1,
        1,
        32,
        6,
        &[0x00ff0000, 0x0000ff00, 0x000000ff, 0],
        &[0; 4],
    ))
    .is_err());
    assert!(decode(&bitfields_bmp(
        1,
        1,
        32,
        6,
        &[0x00ff0000, 0x0000ff00, 0x000000ff, 0x000000ff],
        &[0; 4],
    ))
    .is_err());
    assert!(decode(&bitfields_bmp(
        1,
        1,
        16,
        6,
        &[0xf800, 0x07e0, 0x001f, 0x8000],
        &[0; 4],
    ))
    .is_err());
}
