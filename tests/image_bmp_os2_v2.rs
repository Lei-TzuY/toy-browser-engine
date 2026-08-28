use browser_engine::image::{decode, ImageCache};
use browser_engine::{MemoryLoader, Url};

fn info2_bmp(
    width: u32,
    height: u32,
    depth: u16,
    recording: u16,
    palette: &[[u8; 3]],
    raster: &[u8],
) -> Vec<u8> {
    let offset = 78 + palette.len() * 4;
    let mut out = vec![0u8; offset];
    out[0..2].copy_from_slice(b"BM");
    out[2..6].copy_from_slice(&((offset + raster.len()) as u32).to_le_bytes());
    out[10..14].copy_from_slice(&(offset as u32).to_le_bytes());
    out[14..18].copy_from_slice(&64u32.to_le_bytes());
    out[18..22].copy_from_slice(&width.to_le_bytes());
    out[22..26].copy_from_slice(&height.to_le_bytes());
    out[26..28].copy_from_slice(&1u16.to_le_bytes());
    out[28..30].copy_from_slice(&depth.to_le_bytes());
    out[46..50].copy_from_slice(&(palette.len() as u32).to_le_bytes());
    out[58..60].copy_from_slice(&recording.to_le_bytes());
    for (i, rgb) in palette.iter().enumerate() {
        let base = 78 + i * 4;
        out[base..base + 4].copy_from_slice(&[rgb[2], rgb[1], rgb[0], 0]);
    }
    out.extend_from_slice(raster);
    out
}

#[test]
fn public_decoder_handles_info2_rgb2_palette_and_packed_pixels() {
    let palette = [
        [0, 0, 0],
        [255, 0, 0],
        [0, 255, 0],
        [0, 0, 255],
    ];
    let bytes = info2_bmp(3, 1, 4, 0, &palette, &[0x12, 0x30, 0, 0]);
    let image = decode(&bytes).expect("OS/2 2.x indexed BMP");
    assert_eq!((image.width, image.height), (3, 1));
    assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
    assert_eq!(image.pixel(1, 0), [0, 255, 0, 255]);
    assert_eq!(image.pixel(2, 0), [0, 0, 255, 255]);
}

#[test]
fn public_decoder_honors_bottom_up_and_top_down_recording() {
    let bottom_up = info2_bmp(
        1,
        2,
        24,
        0,
        &[],
        &[
            255, 0, 0, 0, // file row 0: blue (bottom)
            0, 0, 255, 0, // file row 1: red (top)
        ],
    );
    let image = decode(&bottom_up).expect("bottom-up OS/2 2.x BMP");
    assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
    assert_eq!(image.pixel(0, 1), [0, 0, 255, 255]);

    let top_down = info2_bmp(
        1,
        2,
        24,
        1,
        &[],
        &[
            0, 0, 255, 0, // file row 0: red (top)
            255, 0, 0, 0, // file row 1: blue (bottom)
        ],
    );
    let image = decode(&top_down).expect("top-down OS/2 2.x BMP");
    assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
    assert_eq!(image.pixel(0, 1), [0, 0, 255, 255]);
}

#[test]
fn image_cache_decodes_info2_palette_image() {
    let palette = [[0, 0, 0], [255, 0, 0], [0, 255, 0]];
    let bytes = info2_bmp(2, 1, 8, 0, &palette, &[2, 1, 0, 0]);
    let mut loader = MemoryLoader::new();
    loader.insert("demo:///os2-info2.bmp", bytes);
    let url = Url::parse("demo:///os2-info2.bmp").unwrap();
    let mut cache = ImageCache::new();

    let image = cache.fetch(&url, &loader).expect("cached OS/2 2.x BMP");
    assert_eq!(image.pixel(0, 0), [0, 255, 0, 255]);
    assert_eq!(image.pixel(1, 0), [255, 0, 0, 255]);
    assert_eq!(cache.len(), 1);
}

#[test]
fn rejects_invalid_info2_metadata_and_raster() {
    let palette = [[0, 0, 0], [255, 0, 0]];

    let mut compressed = info2_bmp(1, 1, 8, 0, &palette, &[1, 0, 0, 0]);
    compressed[30..34].copy_from_slice(&1u32.to_le_bytes());
    assert!(decode(&compressed).is_err());

    let bad_recording = info2_bmp(1, 1, 8, 2, &palette, &[1, 0, 0, 0]);
    assert!(decode(&bad_recording).is_err());

    let bad_index = info2_bmp(1, 1, 8, 0, &palette, &[7, 0, 0, 0]);
    assert!(decode(&bad_index).is_err());

    let mut truncated = info2_bmp(1, 1, 1, 0, &palette, &[0; 4]);
    truncated.pop();
    assert!(decode(&truncated).is_err());
}
