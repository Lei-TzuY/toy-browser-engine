use browser_engine::image::{decode, ImageCache};
use browser_engine::{MemoryLoader, Url};

fn push_le(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_be(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&value.to_be_bytes());
}

#[test]
fn public_decoder_handles_pfm_endianness_and_bottom_up_rows() {
    let mut bytes = b"PF\n1 2\n-1.0\n".to_vec();
    push_le(&mut bytes, 1.0);
    push_le(&mut bytes, 0.0);
    push_le(&mut bytes, 0.0);
    push_le(&mut bytes, 0.0);
    push_le(&mut bytes, 0.5);
    push_le(&mut bytes, 1.0);

    let image = decode(&bytes).expect("PFM image");
    assert_eq!((image.width, image.height), (1, 2));
    assert_eq!(image.pixel(0, 0), [0, 128, 255, 255]);
    assert_eq!(image.pixel(0, 1), [255, 0, 0, 255]);
}

#[test]
fn public_decoder_handles_big_endian_grayscale() {
    let mut bytes = b"Pf\n1 1\n1.0\n".to_vec();
    push_be(&mut bytes, 0.25);
    let image = decode(&bytes).expect("grayscale PFM image");
    assert_eq!(image.pixel(0, 0), [64, 64, 64, 255]);
}

#[test]
fn image_cache_uses_the_pfm_decoder() {
    let mut bytes = b"Pf\n1 1\n-1.0\n".to_vec();
    push_le(&mut bytes, 0.5);
    let mut loader = MemoryLoader::new();
    loader.insert("demo:///pixel.pfm", bytes);
    let url = Url::parse("demo:///pixel.pfm").unwrap();
    let mut cache = ImageCache::new();

    let image = cache.fetch(&url, &loader).expect("cached PFM");
    assert_eq!(image.pixel(0, 0), [128, 128, 128, 255]);
    assert_eq!(cache.len(), 1);
}

#[test]
fn rejects_invalid_scale_nonfinite_sample_and_truncation() {
    assert!(decode(b"Pf\n1 1\n0\n\0\0\0\0").is_err());

    let mut nonfinite = b"Pf\n1 1\n-1\n".to_vec();
    push_le(&mut nonfinite, f32::INFINITY);
    assert!(decode(&nonfinite).is_err());

    assert!(decode(b"PF\n1 1\n-1\n\0\0\0\0").is_err());
}
