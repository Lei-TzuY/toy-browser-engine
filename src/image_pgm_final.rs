// ============================================================
//  image_pgm_final.rs — Netpbm grayscale extension facade
// ============================================================

use std::collections::HashMap;
use std::rc::Rc;

use crate::net::{LoadError, ResourceLoader, Url};

pub use crate::image_prev2::{ImageError, ImageFormat, RasterImage};

/// Decode image bytes into straight RGBA8, adding Netpbm P2/P5 grayscale
/// support on top of the existing PNG/JPEG/P3/P6 paths.
pub fn decode(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    if bytes.starts_with(b"P2") {
        decode_p2(bytes)
    } else if bytes.starts_with(b"P5") {
        decode_p5(bytes)
    } else {
        crate::image_prev2::decode(bytes)
    }
}

fn decode_p2(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    let mut cursor = 2; // `P2`
    let width = next_ascii_sample(bytes, &mut cursor, "P2")?
        .ok_or_else(|| ImageError::Decode("truncated P2 width".to_string()))?;
    let height = next_ascii_sample(bytes, &mut cursor, "P2")?
        .ok_or_else(|| ImageError::Decode("truncated P2 height".to_string()))?;
    let max = next_ascii_sample(bytes, &mut cursor, "P2")?
        .ok_or_else(|| ImageError::Decode("truncated P2 max value".to_string()))?;

    validate_pgm_header(width, height, max)?;
    let pixel_count = checked_pixel_count(width, height)?;
    let capacity = pixel_count
        .checked_mul(4)
        .ok_or_else(|| ImageError::Decode("PGM dimensions overflow RGBA buffer".to_string()))?;
    let mut pixels = Vec::with_capacity(capacity);

    for _ in 0..pixel_count {
        let sample = next_ascii_sample(bytes, &mut cursor, "P2")?
            .ok_or_else(|| ImageError::Decode("truncated P2 pixel data".to_string()))?;
        let gray = scale_pgm_sample(sample, max)?;
        pixels.extend_from_slice(&[gray, gray, gray, 255]);
    }

    if next_ascii_sample(bytes, &mut cursor, "P2")?.is_some() {
        return Err(ImageError::Decode(
            "P2 contains more samples than its dimensions declare".to_string(),
        ));
    }

    Ok(RasterImage::new(width, height, pixels))
}

fn decode_p5(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    let mut cursor = 2; // `P5`
    let width = next_header_value(bytes, &mut cursor, "width")?;
    let height = next_header_value(bytes, &mut cursor, "height")?;
    let max = next_header_value(bytes, &mut cursor, "max value")?;

    validate_pgm_header(width, height, max)?;
    cursor = binary_raster_start(bytes, cursor)?;

    let pixel_count = checked_pixel_count(width, height)?;
    let sample_bytes = if max < 256 { 1usize } else { 2usize };
    let expected = pixel_count
        .checked_mul(sample_bytes)
        .ok_or_else(|| ImageError::Decode("PGM dimensions overflow pixel buffer".to_string()))?;
    let end = cursor
        .checked_add(expected)
        .ok_or_else(|| ImageError::Decode("PGM pixel offset overflow".to_string()))?;
    if bytes.len() < end {
        return Err(ImageError::Decode("truncated P5 pixel data".to_string()));
    }

    let capacity = pixel_count
        .checked_mul(4)
        .ok_or_else(|| ImageError::Decode("PGM dimensions overflow RGBA buffer".to_string()))?;
    let mut pixels = Vec::with_capacity(capacity);
    let raster = &bytes[cursor..end];

    if sample_bytes == 1 {
        for &sample in raster {
            let gray = scale_pgm_sample(sample as u32, max)?;
            pixels.extend_from_slice(&[gray, gray, gray, 255]);
        }
    } else {
        for sample in raster.chunks_exact(2) {
            let value = u16::from_be_bytes([sample[0], sample[1]]) as u32;
            let gray = scale_pgm_sample(value, max)?;
            pixels.extend_from_slice(&[gray, gray, gray, 255]);
        }
    }

    Ok(RasterImage::new(width, height, pixels))
}

fn validate_pgm_header(width: u32, height: u32, max: u32) -> Result<(), ImageError> {
    if width == 0 || height == 0 {
        return Err(ImageError::Decode(
            "PGM width and height must be non-zero".to_string(),
        ));
    }
    if !(1..=65_535).contains(&max) {
        return Err(ImageError::Decode(format!(
            "PGM max value must be in 1..=65535, got {max}"
        )));
    }
    Ok(())
}

fn checked_pixel_count(width: u32, height: u32) -> Result<usize, ImageError> {
    (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| ImageError::Decode("PGM dimensions overflow pixel count".to_string()))
}

/// Parse one P5 header field while allowing whitespace and comments before the
/// token. The cursor stops on the delimiter after the token so raster-boundary
/// handling can consume exactly one separator without eating a pixel byte.
fn next_header_value(bytes: &[u8], cursor: &mut usize, field: &str) -> Result<u32, ImageError> {
    skip_header_space_and_comments(bytes, cursor);
    if *cursor >= bytes.len() {
        return Err(ImageError::Decode(format!("truncated P5 {field}")));
    }

    let start = *cursor;
    while *cursor < bytes.len() && bytes[*cursor].is_ascii_digit() {
        *cursor += 1;
    }
    if start == *cursor {
        return Err(ImageError::Decode("malformed P5 header".to_string()));
    }
    if *cursor < bytes.len() && !bytes[*cursor].is_ascii_whitespace() {
        return Err(ImageError::Decode("malformed P5 header".to_string()));
    }

    let text = std::str::from_utf8(&bytes[start..*cursor])
        .map_err(|_| ImageError::Decode("non-ASCII P5 header".to_string()))?;
    text.parse::<u32>()
        .map_err(|error| ImageError::Decode(error.to_string()))
}

fn skip_header_space_and_comments(bytes: &[u8], cursor: &mut usize) {
    loop {
        while *cursor < bytes.len() && bytes[*cursor].is_ascii_whitespace() {
            *cursor += 1;
        }
        if *cursor < bytes.len() && bytes[*cursor] == b'#' {
            while *cursor < bytes.len() && bytes[*cursor] != b'\n' && bytes[*cursor] != b'\r' {
                *cursor += 1;
            }
            continue;
        }
        break;
    }
}

/// Consume the single whitespace separator between a P5 maxval and its binary
/// raster. CRLF is treated as one logical separator, matching the hardened P6
/// decoder. Do not skip arbitrary whitespace: a first sample may itself be
/// 0x09, 0x0a, 0x0d or 0x20.
fn binary_raster_start(bytes: &[u8], cursor: usize) -> Result<usize, ImageError> {
    if cursor >= bytes.len() || !bytes[cursor].is_ascii_whitespace() {
        return Err(ImageError::Decode(
            "P5 header must end with whitespace before pixel data".to_string(),
        ));
    }
    if bytes[cursor] == b'\r' && bytes.get(cursor + 1) == Some(&b'\n') {
        Ok(cursor + 2)
    } else {
        Ok(cursor + 1)
    }
}

/// Return the next unsigned decimal token, skipping Netpbm whitespace and
/// comments. P2 permits comments anywhere whitespace is allowed, including
/// between raster samples.
fn next_ascii_sample(
    bytes: &[u8],
    cursor: &mut usize,
    magic: &str,
) -> Result<Option<u32>, ImageError> {
    loop {
        while *cursor < bytes.len() && bytes[*cursor].is_ascii_whitespace() {
            *cursor += 1;
        }
        if *cursor >= bytes.len() {
            return Ok(None);
        }
        if bytes[*cursor] == b'#' {
            while *cursor < bytes.len() && bytes[*cursor] != b'\n' && bytes[*cursor] != b'\r' {
                *cursor += 1;
            }
            continue;
        }
        break;
    }

    let start = *cursor;
    while *cursor < bytes.len() && bytes[*cursor].is_ascii_digit() {
        *cursor += 1;
    }
    if start == *cursor {
        return Err(ImageError::Decode(format!(
            "malformed {magic} token at byte {start}"
        )));
    }
    if *cursor < bytes.len()
        && !bytes[*cursor].is_ascii_whitespace()
        && bytes[*cursor] != b'#'
    {
        return Err(ImageError::Decode(format!(
            "malformed {magic} token at byte {start}"
        )));
    }

    let text = std::str::from_utf8(&bytes[start..*cursor])
        .map_err(|_| ImageError::Decode(format!("non-ASCII {magic} token")))?;
    let value = text
        .parse::<u32>()
        .map_err(|error| ImageError::Decode(error.to_string()))?;
    Ok(Some(value))
}

fn scale_pgm_sample(sample: u32, max: u32) -> Result<u8, ImageError> {
    if sample > max {
        return Err(ImageError::Decode(format!(
            "PGM sample {sample} exceeds max value {max}"
        )));
    }
    Ok(((sample * 255 + max / 2) / max) as u8)
}

/// Decoded images keyed by resolved URL, including failures. This facade keeps
/// P2/P5 support on the same real cache/fetch path used by `<img>` resources.
#[derive(Debug, Default, Clone)]
pub struct ImageCache {
    entries: HashMap<String, Result<Rc<RasterImage>, String>>,
}

impl ImageCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fetch(
        &mut self,
        url: &Url,
        loader: &dyn ResourceLoader,
    ) -> Result<Rc<RasterImage>, String> {
        let key = url.without_fragment().to_string();
        if let Some(entry) = self.entries.get(&key) {
            return entry.clone();
        }
        let outcome = load_and_decode(url, loader);
        self.entries.insert(key, outcome.clone());
        outcome
    }

    pub fn get(&self, url: &Url) -> Option<Rc<RasterImage>> {
        self.entries
            .get(&url.without_fragment().to_string())
            .and_then(|entry| entry.as_ref().ok().cloned())
    }

    pub fn error(&self, url: &Url) -> Option<&str> {
        match self.entries.get(&url.without_fragment().to_string()) {
            Some(Err(message)) => Some(message),
            _ => None,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn insert(&mut self, url: &Url, image: RasterImage) {
        self.entries
            .insert(url.without_fragment().to_string(), Ok(Rc::new(image)));
    }
}

fn load_and_decode(url: &Url, loader: &dyn ResourceLoader) -> Result<Rc<RasterImage>, String> {
    let resource = loader.load(url).map_err(|error: LoadError| error.to_string())?;
    decode(&resource.bytes)
        .map(Rc::new)
        .map_err(|error| format!("{url}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p2_decodes_comments_scaling_and_multiple_pixels() {
        let image = decode(b"P2\n# gray row\n3 1\n100\n0 50 # middle\n100\n").expect("P2 image");
        assert_eq!((image.width, image.height), (3, 1));
        assert_eq!(image.pixel(0, 0), [0, 0, 0, 255]);
        assert_eq!(image.pixel(1, 0), [128, 128, 128, 255]);
        assert_eq!(image.pixel(2, 0), [255, 255, 255, 255]);
    }

    #[test]
    fn p5_decodes_low_and_high_depth_samples() {
        let low = decode(b"P5\n3 1\n100\n\x00\x32\x64").expect("low-max P5");
        assert_eq!(low.pixel(0, 0), [0, 0, 0, 255]);
        assert_eq!(low.pixel(1, 0), [128, 128, 128, 255]);
        assert_eq!(low.pixel(2, 0), [255, 255, 255, 255]);

        let high = decode(b"P5\n3 1\n65535\n\x00\x00\x80\x00\xff\xff").expect("16-bit P5");
        assert_eq!(high.pixel(0, 0), [0, 0, 0, 255]);
        assert_eq!(high.pixel(1, 0), [128, 128, 128, 255]);
        assert_eq!(high.pixel(2, 0), [255, 255, 255, 255]);
    }

    #[test]
    fn p5_crlf_separator_preserves_whitespace_valued_first_sample() {
        let image = decode(b"P5\r\n2 1\r\n255\r\n\x0a\x20").expect("CRLF P5");
        assert_eq!(image.pixel(0, 0), [10, 10, 10, 255]);
        assert_eq!(image.pixel(1, 0), [32, 32, 32, 255]);
    }

    #[test]
    fn pgm_rejects_malformed_dimensions_samples_and_maxval() {
        assert!(matches!(
            decode(b"P2 0 1 255 0"),
            Err(ImageError::Decode(message)) if message.contains("non-zero")
        ));
        assert!(matches!(
            decode(b"P2 1 1 0 0"),
            Err(ImageError::Decode(message)) if message.contains("1..=65535")
        ));
        assert!(matches!(
            decode(b"P2 1 1 10 11"),
            Err(ImageError::Decode(message)) if message.contains("exceeds max value")
        ));
        assert!(matches!(
            decode(b"P2 1 1 255"),
            Err(ImageError::Decode(message)) if message.contains("truncated P2 pixel data")
        ));
        assert!(matches!(
            decode(b"P2 1 1 255 1 2"),
            Err(ImageError::Decode(message)) if message.contains("more samples")
        ));
        assert!(matches!(
            decode(b"P5\n1 1\n100\n\x65"),
            Err(ImageError::Decode(message)) if message.contains("exceeds max value")
        ));
        assert!(matches!(
            decode(b"P5\n1 1\n255\n"),
            Err(ImageError::Decode(message)) if message.contains("truncated P5 pixel data")
        ));
    }
}
