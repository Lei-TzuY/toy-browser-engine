use browser_engine::image::{decode, ImageCache};
use browser_engine::{MemoryLoader, Url};

fn core24(rgb: [u8; 3]) -> Vec<u8> {
    let mut bm = vec![0u8; 26];
    bm[0..2].copy_from_slice(b"BM");
    bm[2..6].copy_from_slice(&30u32.to_le_bytes());
    bm[10..14].copy_from_slice(&26u32.to_le_bytes());
    bm[14..18].copy_from_slice(&12u32.to_le_bytes());
    bm[18..20].copy_from_slice(&1u16.to_le_bytes());
    bm[20..22].copy_from_slice(&1u16.to_le_bytes());
    bm[22..24].copy_from_slice(&1u16.to_le_bytes());
    bm[24..26].copy_from_slice(&24u16.to_le_bytes());
    bm.extend_from_slice(&[rgb[2], rgb[1], rgb[0], 0]);
    bm
}

fn array_entry(mut bm: Vec<u8>, next: u32, absolute_start: usize) -> Vec<u8> {
    let local_off = u32::from_le_bytes(bm[10..14].try_into().unwrap()) as usize;
    bm[10..14].copy_from_slice(&((absolute_start + 14 + local_off) as u32).to_le_bytes());
    let mut out = vec![0u8; 14];
    out[0..2].copy_from_slice(b"BA");
    out[6..10].copy_from_slice(&next.to_le_bytes());
    out.extend_from_slice(&bm);
    out
}

#[test]
fn public_decoder_handles_single_os2_bitmap_array() {
    let bytes = array_entry(core24([21, 43, 65]), 0, 0);
    let image = decode(&bytes).expect("OS/2 bitmap array");
    assert_eq!((image.width, image.height), (1, 1));
    assert_eq!(image.pixel(0, 0), [21, 43, 65, 255]);
}

#[test]
fn public_decoder_uses_first_bitmap_as_array_fallback() {
    let first_len = 14 + core24([255, 0, 0]).len();
    let mut bytes = array_entry(core24([255, 0, 0]), first_len as u32, 0);
    bytes.extend_from_slice(&array_entry(core24([0, 255, 0]), 0, first_len));
    let image = decode(&bytes).expect("multi-entry OS/2 bitmap array");
    assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
}

#[test]
fn image_cache_fetches_os2_bitmap_array() {
    let bytes = array_entry(core24([7, 8, 9]), 0, 0);
    let mut loader = MemoryLoader::new();
    loader.insert("demo:///array.bmp", bytes);
    let url = Url::parse("demo:///array.bmp").unwrap();
    let mut cache = ImageCache::new();
    let image = cache.fetch(&url, &loader).expect("cached OS/2 bitmap array");
    assert_eq!(image.pixel(0, 0), [7, 8, 9, 255]);
    assert_eq!(cache.len(), 1);
}

#[test]
fn rejects_malformed_array_container_metadata() {
    let mut missing_bm = array_entry(core24([1, 2, 3]), 0, 0);
    missing_bm[14..16].copy_from_slice(b"ZZ");
    assert!(decode(&missing_bm).is_err());

    let bad_next = array_entry(core24([1, 2, 3]), 20, 0);
    assert!(decode(&bad_next).is_err());

    let mut missing_next_ba = array_entry(core24([1, 2, 3]), 44, 0);
    missing_next_ba.extend_from_slice(&[0u8; 16]);
    assert!(decode(&missing_next_ba).is_err());

    let mut bad_pixels = array_entry(core24([1, 2, 3]), 0, 0);
    bad_pixels[24..28].copy_from_slice(&4u32.to_le_bytes());
    assert!(decode(&bad_pixels).is_err());
}
