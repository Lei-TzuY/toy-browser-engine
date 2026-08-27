// ============================================================
//  image_ascii_final.rs — ASCII PPM extension facade
// ============================================================

use std::collections::HashMap;
use std::rc::Rc;

use crate::net::{LoadError, ResourceLoader, Url};

pub use crate::image_prev::{ImageError, ImageFormat, RasterImage};

/// Decode image bytes into straight RGBA8, adding Netpbm P3 ASCII PPM support
/// on top of the normalized PNG / hardened P6 implementation.
pub fn decode(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    if bytes.starts_with(b"P3") {
        decode_p3(bytes)
    } else {
        crate::image_prev::decode(bytes)
    }
}

fn decode_p3(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    let mut cursor = 2; // `P3`
    let width = next_ascii_sample(bytes, &mut cursor)?
        .ok_or_else(|| ImageError::Decode("truncated P3 width".to_string()))?;
    let height = next_ascii_sample(bytes, &mut cursor)?
        .ok_or_else(|| ImageError::Decode("truncated P3 height".to_string()))?;
    let max = next_ascii_sample(bytes, &mut cursor)?
        .ok_or_else(|| ImageError::Decode("truncated P3 max value".to_string()))?;

    if width == 0 || height == 0 {
        return Err(ImageError::Decode(
            "PPM width and height must be non-zero".to_string(),
        ));
    }
    if !(1..=65_535).contains(&max) {
        return Err(ImageError::Decode(format!(
            "PPM max value must be in 1..=65535, got {max}"
        )));
    }

    let sample_count = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(3))
        .ok_or_else(|| ImageError::Decode("PPM dimensions overflow sample count".to_string()))?;
    let capacity = (sample_count / 3)
        .checked_mul(4)
        .ok_or_else(|| ImageError::Decode("PPM dimensions overflow RGBA buffer".to_string()))?;
    let mut pixels = Vec::with_capacity(capacity);
    let mut rgb = [0u8; 3];

    for index in 0..sample_count {
        let sample = next_ascii_sample(bytes, &mut cursor)?
            .ok_or_else(|| ImageError::Decode("truncated P3 pixel data".to_string()))?;
        rgb[index % 3] = scale_ppm_sample(sample, max)?;
        if index % 3 == 2 {
            pixels.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
    }

    // Only whitespace/comments may follow the declared raster. Reject an extra
    // numeric sample rather than silently accepting an incorrectly-sized image.
    if next_ascii_sample(bytes, &mut cursor)?.is_some() {
        return Err(ImageError::Decode(
            "P3 contains more samples than its dimensions declare".to_string(),
        ));
    }

    Ok(RasterImage::new(width, height, pixels))
}

/// Return the next unsigned decimal token, skipping Netpbm whitespace and
/// `# ... end-of-line` comments. P3 permits comments anywhere whitespace is
/// permitted, including between raster samples.
fn next_ascii_sample(bytes: &[u8], cursor: &mut usize) -> Result<Option<u32>, ImageError> {
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
            "malformed P3 token at byte {start}"
        )));
    }
    if *cursor < bytes.len()
        && !bytes[*cursor].is_ascii_whitespace()
        && bytes[*cursor] != b'#'
    {
        return Err(ImageError::Decode(format!(
            "malformed P3 token at byte {start}"
        )));
    }

    let text = std::str::from_utf8(&bytes[start..*cursor])
        .map_err(|_| ImageError::Decode("non-ASCII P3 token".to_string()))?;
    let value = text
        .parse::<u32>()
        .map_err(|error| ImageError::Decode(error.to_string()))?;
    Ok(Some(value))
}

fn scale_ppm_sample(sample: u32, max: u32) -> Result<u8, ImageError> {
    if sample > max {
        return Err(ImageError::Decode(format!(
            "PPM sample {sample} exceeds max value {max}"
        )));
    }
    Ok(((sample * 255 + max / 2) / max) as u8)
}

/// Decoded images keyed by resolved URL, including failures. This facade keeps
/// P3 support on the same real cache/fetch path used by `<img>` resources.
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
    fn p3_decodes_comments_scaling_and_multiple_pixels() {
        let image = decode(
            b"P3\n# dimensions\n2 1\n100\n0 50 100  # between pixels\n100 0 25\n",
        )
        .expect("P3 image");
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.pixel(0, 0), [0, 128, 255, 255]);
        assert_eq!(image.pixel(1, 0), [255, 0, 64, 255]);
    }

    #[test]
    fn p3_rejects_wrong_sample_count_and_out_of_range_samples() {
        assert!(matches!(
            decode(b"P3 1 1 255 1 2"),
            Err(ImageError::Decode(message)) if message.contains("truncated P3 pixel data")
        ));
        assert!(matches!(
            decode(b"P3 1 1 10 0 11 0"),
            Err(ImageError::Decode(message)) if message.contains("exceeds max value")
        ));
        assert!(matches!(
            decode(b"P3 1 1 255 1 2 3 4"),
            Err(ImageError::Decode(message)) if message.contains("more samples")
        ));
    }
}
