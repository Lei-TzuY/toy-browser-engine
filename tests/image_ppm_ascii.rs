use browser_engine::image::{decode, ImageCache};
use browser_engine::net::{MemoryLoader, Url};

#[test]
fn public_decoder_accepts_ascii_ppm_with_16_bit_domain() {
    let image = decode(b"P3\n1 1\n65535\n0 32768 65535\n").expect("P3 decode");
    assert_eq!((image.width, image.height), (1, 1));
    assert_eq!(image.pixel(0, 0), [0, 128, 255, 255]);
}

#[test]
fn image_cache_fetches_and_decodes_ascii_ppm() {
    let mut loader = MemoryLoader::new();
    loader.insert(
        "demo:///ascii.ppm",
        b"P3\n# comment before dimensions\n2 1\n10\n10 0 0 0 10 0\n".to_vec(),
    );
    let url = Url::parse("demo:///ascii.ppm").unwrap();
    let mut cache = ImageCache::new();

    let first = cache.fetch(&url, &loader).expect("cached P3");
    assert_eq!(first.pixel(0, 0), [255, 0, 0, 255]);
    assert_eq!(first.pixel(1, 0), [0, 255, 0, 255]);

    let second = cache.fetch(&url, &loader).expect("cache hit");
    assert!(std::rc::Rc::ptr_eq(&first, &second));
}

#[test]
fn ascii_ppm_rejects_non_numeric_raster_garbage() {
    let error = decode(b"P3 1 1 255 1 x 3").expect_err("malformed P3 must fail");
    assert!(error.to_string().contains("malformed P3 token"));
}
