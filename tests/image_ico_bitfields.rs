use browser_engine::image::{decode, ImageCache};
use browser_engine::{MemoryLoader, Url};

fn masked_ico(bit_depth: u16, compression: u32, masks: &[u32], pixel: u32, and_mask: u8) -> Vec<u8> {
    let mut dib = vec![0u8; 40 + masks.len() * 4];
    dib[0..4].copy_from_slice(&40u32.to_le_bytes());
    dib[4..8].copy_from_slice(&1i32.to_le_bytes());
    dib[8..12].copy_from_slice(&2i32.to_le_bytes());
    dib[12..14].copy_from_slice(&1u16.to_le_bytes());
    dib[14..16].copy_from_slice(&bit_depth.to_le_bytes());
    dib[16..20].copy_from_slice(&compression.to_le_bytes());
    for (i, mask) in masks.iter().enumerate() {
        dib[40 + i * 4..44 + i * 4].copy_from_slice(&mask.to_le_bytes());
    }
    if bit_depth == 16 {
        dib.extend_from_slice(&(pixel as u16).to_le_bytes());
        dib.extend_from_slice(&[0, 0]);
    } else {
        dib.extend_from_slice(&pixel.to_le_bytes());
    }
    dib.extend_from_slice(&[and_mask, 0, 0, 0]);

    let mut out = vec![0u8; 22];
    out[2..4].copy_from_slice(&1u16.to_le_bytes());
    out[4..6].copy_from_slice(&1u16.to_le_bytes());
    out[6] = 1;
    out[7] = 1;
    out[10..12].copy_from_slice(&1u16.to_le_bytes());
    out[12..14].copy_from_slice(&bit_depth.to_le_bytes());
    out[14..18].copy_from_slice(&(dib.len() as u32).to_le_bytes());
    out[18..22].copy_from_slice(&22u32.to_le_bytes());
    out.extend_from_slice(&dib);
    out
}

#[test]
fn public_decoder_handles_rgb565_ico_bitfields() {
    let bytes = masked_ico(16, 3, &[0xf800, 0x07e0, 0x001f], 0x07e0, 0);
    let image = decode(&bytes).expect("RGB565 ICO");
    assert_eq!(image.pixel(0, 0), [0, 255, 0, 255]);
}

#[test]
fn public_decoder_preserves_explicit_alpha_mask() {
    let bytes = masked_ico(
        32,
        6,
        &[0x00ff0000, 0x0000ff00, 0x000000ff, 0xff000000],
        0x80402010,
        0x80,
    );
    let image = decode(&bytes).expect("alpha-bitfields ICO");
    assert_eq!(image.pixel(0, 0), [64, 32, 16, 128]);
}

#[test]
fn image_cache_fetches_masked_ico() {
    let bytes = masked_ico(16, 3, &[0xf800, 0x07e0, 0x001f], 0xf800, 0);
    let mut loader = MemoryLoader::new();
    loader.insert("demo:///masked.ico", bytes);
    let url = Url::parse("demo:///masked.ico").unwrap();
    let mut cache = ImageCache::new();
    let image = cache.fetch(&url, &loader).expect("cached masked ICO");
    assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
    assert_eq!(cache.len(), 1);
}

#[test]
fn rejects_invalid_mask_layouts_and_missing_mask_data() {
    let overlapping = masked_ico(16, 3, &[0x7c00, 0x7c00, 0x001f], 0, 0);
    assert!(decode(&overlapping).is_err());

    let mut truncated = masked_ico(16, 3, &[0xf800, 0x07e0, 0x001f], 0, 0);
    truncated.truncate(22 + 44);
    truncated[14..18].copy_from_slice(&22u32.to_le_bytes());
    assert!(decode(&truncated).is_err());
}
