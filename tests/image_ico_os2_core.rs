use browser_engine::image::{decode, ImageCache};
use browser_engine::{MemoryLoader, Url};

fn palette(size: usize) -> Vec<[u8; 3]> {
    let mut entries = vec![[0, 0, 0]; size];
    if size > 1 {
        entries[1] = [255, 0, 0];
    }
    if size > 2 {
        entries[2] = [0, 255, 0];
    }
    entries
}

fn core_ico(
    width: u16,
    height: u16,
    depth: u16,
    palette: &[[u8; 3]],
    xor: &[u8],
    and_mask: &[u8],
) -> Vec<u8> {
    let mut dib = vec![0u8; 12 + palette.len() * 3];
    dib[0..4].copy_from_slice(&12u32.to_le_bytes());
    dib[4..6].copy_from_slice(&width.to_le_bytes());
    dib[6..8].copy_from_slice(&height.saturating_mul(2).to_le_bytes());
    dib[8..10].copy_from_slice(&1u16.to_le_bytes());
    dib[10..12].copy_from_slice(&depth.to_le_bytes());
    for (index, rgb) in palette.iter().enumerate() {
        let base = 12 + index * 3;
        dib[base..base + 3].copy_from_slice(&[rgb[2], rgb[1], rgb[0]]);
    }
    dib.extend_from_slice(xor);
    dib.extend_from_slice(and_mask);

    let mut out = vec![0u8; 22];
    out[2..4].copy_from_slice(&1u16.to_le_bytes());
    out[4..6].copy_from_slice(&1u16.to_le_bytes());
    out[6] = if width == 256 { 0 } else { width as u8 };
    out[7] = if height == 256 { 0 } else { height as u8 };
    out[10..12].copy_from_slice(&1u16.to_le_bytes());
    out[12..14].copy_from_slice(&depth.to_le_bytes());
    out[14..18].copy_from_slice(&(dib.len() as u32).to_le_bytes());
    out[18..22].copy_from_slice(&22u32.to_le_bytes());
    out.extend_from_slice(&dib);
    out
}

#[test]
fn public_decoder_handles_8bit_os2_core_ico() {
    let p = palette(256);
    let bytes = core_ico(2, 1, 8, &p, &[2, 1, 0, 0], &[0, 0, 0, 0]);
    let image = decode(&bytes).expect("8-bit OS/2 core ICO");
    assert_eq!(image.pixel(0, 0), [0, 255, 0, 255]);
    assert_eq!(image.pixel(1, 0), [255, 0, 0, 255]);
}

#[test]
fn public_decoder_applies_os2_core_and_mask() {
    let bytes = core_ico(
        1,
        1,
        24,
        &[],
        &[0, 0, 255, 0],
        &[0x80, 0, 0, 0],
    );
    assert_eq!(decode(&bytes).unwrap().pixel(0, 0), [255, 0, 0, 0]);
}

#[test]
fn image_cache_fetches_os2_core_ico() {
    let p = palette(16);
    let bytes = core_ico(2, 1, 4, &p, &[0x12, 0, 0, 0], &[0, 0, 0, 0]);
    let mut loader = MemoryLoader::new();
    loader.insert("demo:///os2-core.ico", bytes);
    let url = Url::parse("demo:///os2-core.ico").unwrap();
    let mut cache = ImageCache::new();
    let image = cache.fetch(&url, &loader).expect("cached OS/2 core ICO");
    assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
    assert_eq!(image.pixel(1, 0), [0, 255, 0, 255]);
    assert_eq!(cache.len(), 1);
}

#[test]
fn rejects_directory_mismatch_and_truncated_core_palette() {
    let p = palette(2);
    let mut mismatch = core_ico(1, 1, 1, &p, &[0; 4], &[0; 4]);
    mismatch[6] = 2;
    assert!(decode(&mismatch).is_err());

    let mut truncated = core_ico(1, 1, 1, &p, &[0; 4], &[0; 4]);
    truncated.truncate(12 + 22 + 3);
    let new_size = (truncated.len() - 22) as u32;
    truncated[14..18].copy_from_slice(&new_size.to_le_bytes());
    assert!(decode(&truncated).is_err());
}
