use browser_engine::image::{decode, ImageCache};
use browser_engine::{MemoryLoader, Url};

#[test]
fn public_decoder_accepts_rgb_alpha_pam() {
    let image = decode(
        b"P7\nWIDTH 1\nHEIGHT 1\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n\x10\x20\x30\x40",
    )
    .expect("RGB-alpha PAM");
    assert_eq!((image.width, image.height), (1, 1));
    assert_eq!(image.pixel(0, 0), [0x10, 0x20, 0x30, 0x40]);
}

#[test]
fn public_decoder_accepts_sixteen_bit_grayscale_pam() {
    let image = decode(
        b"P7\nWIDTH 2\nHEIGHT 1\nDEPTH 1\nMAXVAL 65535\nTUPLTYPE GRAYSCALE\nENDHDR\n\x00\x00\xff\xff",
    )
    .expect("16-bit grayscale PAM");
    assert_eq!(image.pixel(0, 0), [0, 0, 0, 255]);
    assert_eq!(image.pixel(1, 0), [255, 255, 255, 255]);
}

#[test]
fn image_cache_uses_the_pam_decoder() {
    let mut loader = MemoryLoader::new();
    loader.insert(
        "demo:///pixel.pam",
        b"P7\nWIDTH 1\nHEIGHT 1\nDEPTH 2\nMAXVAL 100\nTUPLTYPE GRAYSCALE_ALPHA\nENDHDR\n\x32\x19".to_vec(),
    );
    let url = Url::parse("demo:///pixel.pam").unwrap();
    let mut cache = ImageCache::new();

    let image = cache.fetch(&url, &loader).expect("cached PAM");
    assert_eq!(image.pixel(0, 0), [128, 128, 128, 64]);
    assert_eq!(cache.len(), 1);
}
