use browser_engine::image::{decode, ImageCache};
use browser_engine::{MemoryLoader, Url};

#[test]
fn public_decoder_accepts_ascii_p1() {
    let image = decode(b"P1\n4 1\n0 1 1 0\n").expect("P1 image");
    assert_eq!((image.width, image.height), (4, 1));
    assert_eq!(image.pixel(0, 0), [255, 255, 255, 255]);
    assert_eq!(image.pixel(1, 0), [0, 0, 0, 255]);
    assert_eq!(image.pixel(2, 0), [0, 0, 0, 255]);
    assert_eq!(image.pixel(3, 0), [255, 255, 255, 255]);
}

#[test]
fn public_decoder_accepts_binary_p4_with_padding() {
    let image = decode(b"P4\n10 1\n\xaa\xbf").expect("P4 image");
    assert_eq!((image.width, image.height), (10, 1));
    let expected = [0u8, 255, 0, 255, 0, 255, 0, 255, 0, 255];
    for (x, gray) in expected.into_iter().enumerate() {
        assert_eq!(image.pixel(x as u32, 0), [gray, gray, gray, 255]);
    }
}

#[test]
fn image_cache_uses_the_pbm_decoder() {
    let mut loader = MemoryLoader::new();
    loader.insert("demo:///bitmap.pbm", b"P4\n8 1\n\x81".to_vec());
    let url = Url::parse("demo:///bitmap.pbm").unwrap();
    let mut cache = ImageCache::new();
    let image = cache.fetch(&url, &loader).expect("cached PBM");
    assert_eq!(image.pixel(0, 0), [0, 0, 0, 255]);
    assert_eq!(image.pixel(7, 0), [0, 0, 0, 255]);
    assert_eq!(cache.len(), 1);
}
