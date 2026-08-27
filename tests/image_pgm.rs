use browser_engine::image::{decode, ImageCache};
use browser_engine::{MemoryLoader, Url};

#[test]
fn public_decoder_accepts_ascii_p2_with_comments() {
    let image = decode(b"P2\n# row\n3 1\n100\n0 50 100\n").expect("P2 image");
    assert_eq!((image.width, image.height), (3, 1));
    assert_eq!(image.pixel(0, 0), [0, 0, 0, 255]);
    assert_eq!(image.pixel(1, 0), [128, 128, 128, 255]);
    assert_eq!(image.pixel(2, 0), [255, 255, 255, 255]);
}

#[test]
fn public_decoder_accepts_binary_p5_sixteen_bit_samples() {
    let image = decode(b"P5\n3 1\n65535\n\x00\x00\x80\x00\xff\xff")
        .expect("16-bit P5 image");
    assert_eq!(image.pixel(0, 0), [0, 0, 0, 255]);
    assert_eq!(image.pixel(1, 0), [128, 128, 128, 255]);
    assert_eq!(image.pixel(2, 0), [255, 255, 255, 255]);
}

#[test]
fn image_cache_uses_the_pgm_decoder() {
    let mut loader = MemoryLoader::new();
    loader.insert("demo:///gray.pgm", b"P5\n2 1\n255\n\x20\xff".to_vec());
    let url = Url::parse("demo:///gray.pgm").unwrap();
    let mut cache = ImageCache::new();

    let image = cache.fetch(&url, &loader).expect("cached PGM");
    assert_eq!(image.pixel(0, 0), [32, 32, 32, 255]);
    assert_eq!(image.pixel(1, 0), [255, 255, 255, 255]);
    assert_eq!(cache.len(), 1);
}
