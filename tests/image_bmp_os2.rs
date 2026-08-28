use browser_engine::image::{decode, ImageCache};
use browser_engine::{MemoryLoader, Url};

fn core_bmp(width: u16, height: u16, depth: u16, palette: &[[u8; 3]], raster: &[u8]) -> Vec<u8> {
    let offset = 26 + palette.len() * 3;
    let mut out = vec![0u8; offset];
    out[0..2].copy_from_slice(b"BM");
    out[2..6].copy_from_slice(&((offset + raster.len()) as u32).to_le_bytes());
    out[10..14].copy_from_slice(&(offset as u32).to_le_bytes());
    out[14..18].copy_from_slice(&12u32.to_le_bytes());
    out[18..20].copy_from_slice(&width.to_le_bytes());
    out[20..22].copy_from_slice(&height.to_le_bytes());
    out[22..24].copy_from_slice(&1u16.to_le_bytes());
    out[24..26].copy_from_slice(&depth.to_le_bytes());
    for (i, rgb) in palette.iter().enumerate() {
        let base = 26 + i * 3;
        out[base..base + 3].copy_from_slice(&[rgb[2], rgb[1], rgb[0]]);
    }
    out.extend_from_slice(raster);
    out
}

fn indexed_palette(size: usize) -> Vec<[u8; 3]> {
    let mut palette = vec![[0, 0, 0]; size];
    if size > 1 { palette[1] = [255, 0, 0]; }
    if size > 2 { palette[2] = [0, 255, 0]; }
    if size > 3 { palette[3] = [0, 0, 255]; }
    palette
}

#[test]
fn public_decoder_handles_coreheader_24_bit_bottom_up_rows() {
    let bytes = core_bmp(
        2,
        2,
        24,
        &[],
        &[
            255, 0, 0, 0, 255, 0, 0, 0, // bottom: blue, green, pad
            0, 0, 255, 255, 255, 255, 0, 0, // top: red, white, pad
        ],
    );
    let image = decode(&bytes).expect("OS/2 24-bit core BMP");
    assert_eq!((image.width, image.height), (2, 2));
    assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
    assert_eq!(image.pixel(1, 0), [255, 255, 255, 255]);
    assert_eq!(image.pixel(0, 1), [0, 0, 255, 255]);
    assert_eq!(image.pixel(1, 1), [0, 255, 0, 255]);
}

#[test]
fn public_decoder_handles_packed_coreheader_palette_pixels() {
    let p1 = indexed_palette(2);
    let one = core_bmp(3, 1, 1, &p1, &[0b0100_0000, 0, 0, 0]);
    let image = decode(&one).expect("OS/2 1-bit core BMP");
    assert_eq!(image.pixel(0, 0), [0, 0, 0, 255]);
    assert_eq!(image.pixel(1, 0), [255, 0, 0, 255]);
    assert_eq!(image.pixel(2, 0), [0, 0, 0, 255]);

    let p4 = indexed_palette(16);
    let four = core_bmp(3, 1, 4, &p4, &[0x12, 0x30, 0, 0]);
    let image = decode(&four).expect("OS/2 4-bit core BMP");
    assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
    assert_eq!(image.pixel(1, 0), [0, 255, 0, 255]);
    assert_eq!(image.pixel(2, 0), [0, 0, 255, 255]);
}

#[test]
fn image_cache_decodes_rgbtriple_palette() {
    let palette = indexed_palette(256);
    let bytes = core_bmp(3, 1, 8, &palette, &[3, 2, 1, 0]);
    let mut loader = MemoryLoader::new();
    loader.insert("demo:///os2-core.bmp", bytes);
    let url = Url::parse("demo:///os2-core.bmp").unwrap();
    let mut cache = ImageCache::new();

    let image = cache.fetch(&url, &loader).expect("cached OS/2 core BMP");
    assert_eq!(image.pixel(0, 0), [0, 0, 255, 255]);
    assert_eq!(image.pixel(1, 0), [0, 255, 0, 255]);
    assert_eq!(image.pixel(2, 0), [255, 0, 0, 255]);
    assert_eq!(cache.len(), 1);
}

#[test]
fn rejects_unsupported_depth_truncated_palette_and_raster() {
    assert!(decode(&core_bmp(1, 1, 2, &[], &[0; 4])).is_err());

    let mut bad_palette = core_bmp(1, 1, 4, &indexed_palette(16), &[0; 4]);
    bad_palette[10..14].copy_from_slice(&30u32.to_le_bytes());
    assert!(decode(&bad_palette).is_err());

    let mut truncated = core_bmp(1, 1, 1, &indexed_palette(2), &[0; 4]);
    truncated.pop();
    assert!(decode(&truncated).is_err());
}
