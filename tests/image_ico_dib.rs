use browser_engine::image::{decode, ImageCache};
use browser_engine::{MemoryLoader, Url};

fn ico_dib32(width: u8, height: u8, pixels_top_down: &[[u8; 4]], and_rows: &[u8]) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    assert_eq!(pixels_top_down.len(), w * h);
    let and_stride = ((w + 31) / 32) * 4;
    assert_eq!(and_rows.len(), and_stride * h);

    let mut dib = vec![0u8; 40];
    dib[0..4].copy_from_slice(&40u32.to_le_bytes());
    dib[4..8].copy_from_slice(&(width as i32).to_le_bytes());
    dib[8..12].copy_from_slice(&((height as i32) * 2).to_le_bytes());
    dib[12..14].copy_from_slice(&1u16.to_le_bytes());
    dib[14..16].copy_from_slice(&32u16.to_le_bytes());
    for y in (0..h).rev() {
        for x in 0..w {
            let [r, g, b, a] = pixels_top_down[y * w + x];
            dib.extend_from_slice(&[b, g, r, a]);
        }
    }
    dib.extend_from_slice(and_rows);

    let mut out = vec![0u8; 22];
    out[2..4].copy_from_slice(&1u16.to_le_bytes());
    out[4..6].copy_from_slice(&1u16.to_le_bytes());
    out[6] = width;
    out[7] = height;
    out[10..12].copy_from_slice(&1u16.to_le_bytes());
    out[12..14].copy_from_slice(&32u16.to_le_bytes());
    out[14..18].copy_from_slice(&(dib.len() as u32).to_le_bytes());
    out[18..22].copy_from_slice(&22u32.to_le_bytes());
    out.extend_from_slice(&dib);
    out
}

#[test]
fn public_decoder_handles_dib_backed_favicon() {
    let bytes = ico_dib32(
        2,
        1,
        &[[12, 34, 56, 255], [78, 90, 123, 64]],
        &[0; 4],
    );
    let image = decode(&bytes).expect("DIB-backed ICO");
    assert_eq!((image.width, image.height), (2, 1));
    assert_eq!(image.pixel(0, 0), [12, 34, 56, 255]);
    assert_eq!(image.pixel(1, 0), [78, 90, 123, 64]);
}

#[test]
fn public_decoder_applies_legacy_and_mask_fallback() {
    let mut mask = vec![0u8; 4];
    mask[0] = 0b0010_0000; // x=2 transparent.
    let bytes = ico_dib32(
        3,
        1,
        &[
            [1, 2, 3, 0],
            [4, 5, 6, 0],
            [7, 8, 9, 0],
        ],
        &mask,
    );
    let image = decode(&bytes).expect("AND-mask ICO");
    assert_eq!(image.pixel(0, 0), [1, 2, 3, 255]);
    assert_eq!(image.pixel(1, 0), [4, 5, 6, 255]);
    assert_eq!(image.pixel(2, 0), [7, 8, 9, 0]);
}

#[test]
fn image_cache_fetches_dib_backed_favicon() {
    let bytes = ico_dib32(1, 1, &[[9, 8, 7, 111]], &[0; 4]);
    let mut loader = MemoryLoader::new();
    loader.insert("demo:///legacy.ico", bytes);
    let url = Url::parse("demo:///legacy.ico").unwrap();
    let mut cache = ImageCache::new();
    let image = cache.fetch(&url, &loader).expect("cached DIB ICO");
    assert_eq!(image.pixel(0, 0), [9, 8, 7, 111]);
    assert_eq!(cache.len(), 1);
}

#[test]
fn rejects_invalid_dib_depth_and_truncated_and_mask() {
    let mut wrong_depth = ico_dib32(1, 1, &[[1, 2, 3, 4]], &[0; 4]);
    wrong_depth[22 + 14..22 + 16].copy_from_slice(&24u16.to_le_bytes());
    assert!(decode(&wrong_depth).is_err());

    let mut truncated = ico_dib32(1, 1, &[[1, 2, 3, 4]], &[0; 4]);
    truncated.pop();
    truncated[14..18].copy_from_slice(&((truncated.len() - 22) as u32).to_le_bytes());
    assert!(decode(&truncated).is_err());
}
