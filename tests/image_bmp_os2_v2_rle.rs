use browser_engine::image::{decode, ImageCache};
use browser_engine::{MemoryLoader, Url};

fn bmp(width: u32, height: u32, depth: u16, compression: u32, palette: &[[u8; 3]], stream: &[u8]) -> Vec<u8> {
    let offset = 78 + palette.len() * 4;
    let mut out = vec![0u8; offset];
    out[0..2].copy_from_slice(b"BM");
    out[2..6].copy_from_slice(&((offset + stream.len()) as u32).to_le_bytes());
    out[10..14].copy_from_slice(&(offset as u32).to_le_bytes());
    out[14..18].copy_from_slice(&64u32.to_le_bytes());
    out[18..22].copy_from_slice(&width.to_le_bytes());
    out[22..26].copy_from_slice(&height.to_le_bytes());
    out[26..28].copy_from_slice(&1u16.to_le_bytes());
    out[28..30].copy_from_slice(&depth.to_le_bytes());
    out[30..34].copy_from_slice(&compression.to_le_bytes());
    out[46..50].copy_from_slice(&(palette.len() as u32).to_le_bytes());
    for (i, rgb) in palette.iter().enumerate() {
        let base = 78 + i * 4;
        out[base..base + 4].copy_from_slice(&[rgb[2], rgb[1], rgb[0], 0]);
    }
    out.extend_from_slice(stream);
    out
}

#[test]
fn public_decoder_handles_os2_rle8_mixed_commands() {
    let palette = [[0,0,0],[255,0,0],[0,255,0],[0,0,255]];
    let bytes = bmp(4, 2, 8, 1, &palette, &[
        4, 3, 0, 0,
        0, 4, 1, 2, 1, 2, 0, 0,
        0, 1,
    ]);
    let image = decode(&bytes).expect("OS/2 RLE8");
    assert_eq!((image.width, image.height), (4, 2));
    assert_eq!(image.pixel(0, 0), [255,0,0,255]);
    assert_eq!(image.pixel(1, 0), [0,255,0,255]);
    assert_eq!(image.pixel(0, 1), [0,0,255,255]);
}

#[test]
fn public_decoder_handles_os2_rle4_absolute_and_delta() {
    let palette = [[0,0,0],[255,0,0],[0,255,0],[0,0,255]];
    let bytes = bmp(5, 2, 4, 2, &palette, &[
        5, 0x12, 0, 0,
        0, 2, 1, 0,
        0, 4, 0x31, 0x23, 0, 0,
        0, 1,
    ]);
    let image = decode(&bytes).expect("OS/2 RLE4");
    assert_eq!(image.pixel(0, 0), [0,0,0,255]);
    assert_eq!(image.pixel(1, 0), [0,0,255,255]);
    assert_eq!(image.pixel(0, 1), [255,0,0,255]);
    assert_eq!(image.pixel(1, 1), [0,255,0,255]);
}

#[test]
fn image_cache_fetches_os2_rle8() {
    let palette = [[0,0,0],[255,0,0]];
    let bytes = bmp(3, 1, 8, 1, &palette, &[3, 1, 0, 1]);
    let mut loader = MemoryLoader::new();
    loader.insert("demo:///os2-rle8.bmp", bytes);
    let url = Url::parse("demo:///os2-rle8.bmp").unwrap();
    let mut cache = ImageCache::new();
    let image = cache.fetch(&url, &loader).expect("cached OS/2 RLE8");
    assert_eq!(image.pixel(2, 0), [255,0,0,255]);
    assert_eq!(cache.len(), 1);
}

#[test]
fn rejects_malformed_os2_rle_streams() {
    let palette = [[0,0,0],[255,0,0]];
    assert!(decode(&bmp(1,1,8,1,&palette,&[1,1])).is_err());
    assert!(decode(&bmp(1,1,8,1,&palette,&[2,1,0,1])).is_err());
    assert!(decode(&bmp(1,1,4,2,&palette,&[1,0xf0,0,1])).is_err());
    let mut top_down = bmp(1,1,8,1,&palette,&[1,1,0,1]);
    top_down[58..60].copy_from_slice(&1u16.to_le_bytes());
    assert!(decode(&top_down).is_err());
}
