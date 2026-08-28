use browser_engine::image::{decode, ImageCache};
use browser_engine::{MemoryLoader, Url};

fn ico(depth: u16, payload: Vec<u8>) -> Vec<u8> {
    let mut out = vec![0u8; 22];
    out[2..4].copy_from_slice(&1u16.to_le_bytes());
    out[4..6].copy_from_slice(&1u16.to_le_bytes());
    out[6] = 1;
    out[7] = 1;
    out[10..12].copy_from_slice(&1u16.to_le_bytes());
    out[12..14].copy_from_slice(&depth.to_le_bytes());
    out[14..18].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    out[18..22].copy_from_slice(&22u32.to_le_bytes());
    out.extend_from_slice(&payload);
    out
}

fn rgb565_with_external_masks(and_transparent: bool) -> Vec<u8> {
    let mut out = vec![0u8; 40 + 12 + 4 + 4];
    out[0..4].copy_from_slice(&40u32.to_le_bytes());
    out[4..8].copy_from_slice(&1i32.to_le_bytes());
    out[8..12].copy_from_slice(&2i32.to_le_bytes());
    out[12..14].copy_from_slice(&1u16.to_le_bytes());
    out[14..16].copy_from_slice(&16u16.to_le_bytes());
    out[16..20].copy_from_slice(&3u32.to_le_bytes());
    out[40..44].copy_from_slice(&0xf800u32.to_le_bytes());
    out[44..48].copy_from_slice(&0x07e0u32.to_le_bytes());
    out[48..52].copy_from_slice(&0x001fu32.to_le_bytes());
    out[52..54].copy_from_slice(&0xf800u16.to_le_bytes());
    if and_transparent {
        out[56] = 0x80;
    }
    out
}

fn alpha_bitfields_v3() -> Vec<u8> {
    let mut out = vec![0u8; 56 + 4 + 4];
    out[0..4].copy_from_slice(&56u32.to_le_bytes());
    out[4..8].copy_from_slice(&1i32.to_le_bytes());
    out[8..12].copy_from_slice(&2i32.to_le_bytes());
    out[12..14].copy_from_slice(&1u16.to_le_bytes());
    out[14..16].copy_from_slice(&32u16.to_le_bytes());
    out[16..20].copy_from_slice(&6u32.to_le_bytes());
    out[40..44].copy_from_slice(&0x00ff0000u32.to_le_bytes());
    out[44..48].copy_from_slice(&0x0000ff00u32.to_le_bytes());
    out[48..52].copy_from_slice(&0x000000ffu32.to_le_bytes());
    out[52..56].copy_from_slice(&0xff000000u32.to_le_bytes());
    out[56..60].copy_from_slice(&[0x33, 0x22, 0x11, 0x80]);
    // Deliberately mark the AND bit transparent. Explicit alpha is authoritative.
    out[60] = 0x80;
    out
}

#[test]
fn public_decoder_handles_rgb565_bitfield_ico_and_mask() {
    let image = decode(&ico(16, rgb565_with_external_masks(true))).expect("RGB565 ICO");
    assert_eq!((image.width, image.height), (1, 1));
    assert_eq!(image.pixel(0, 0), [255, 0, 0, 0]);
}

#[test]
fn public_decoder_handles_embedded_alpha_masks() {
    let image = decode(&ico(32, alpha_bitfields_v3())).expect("alpha-bitfield ICO");
    assert_eq!(image.pixel(0, 0), [0x11, 0x22, 0x33, 0x80]);
}

#[test]
fn image_cache_fetches_bitfield_backed_favicon() {
    let bytes = ico(16, rgb565_with_external_masks(false));
    let mut loader = MemoryLoader::new();
    loader.insert("demo:///masked.ico", bytes);
    let url = Url::parse("demo:///masked.ico").unwrap();
    let mut cache = ImageCache::new();
    let image = cache.fetch(&url, &loader).expect("cached bitfield ICO");
    assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
    assert_eq!(cache.len(), 1);
}

#[test]
fn rejects_overlapping_masks_via_existing_bmp_decoder() {
    let mut dib = rgb565_with_external_masks(false);
    dib[44..48].copy_from_slice(&0xf800u32.to_le_bytes());
    assert!(decode(&ico(16, dib)).is_err());
}
