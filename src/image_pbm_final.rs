// ============================================================
//  image_pbm_final.rs — Netpbm bitmap extension facade
// ============================================================

use std::collections::HashMap;
use std::rc::Rc;

use crate::net::{LoadError, ResourceLoader, Url};

pub use crate::image_prev3::{ImageError, ImageFormat, RasterImage};

/// Decode image bytes into straight RGBA8, adding Netpbm P1/P4 bilevel bitmap
/// support on top of the existing PNG/JPEG/PPM/PGM paths.
pub fn decode(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    if bytes.starts_with(b"P1") {
        decode_p1(bytes)
    } else if bytes.starts_with(b"P4") {
        decode_p4(bytes)
    } else {
        crate::image_prev3::decode(bytes)
    }
}

fn decode_p1(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    let mut cursor = 2; // `P1`
    let width = next_ascii_bit_token(bytes, &mut cursor)?
        .ok_or_else(|| ImageError::Decode("truncated P1 width".to_string()))?;
    let height = next_ascii_bit_token(bytes, &mut cursor)?
        .ok_or_else(|| ImageError::Decode("truncated P1 height".to_string()))?;
    validate_dimensions(width, height)?;

    let pixel_count = checked_pixel_count(width, height)?;
    let capacity = pixel_count
        .checked_mul(4)
        .ok_or_else(|| ImageError::Decode("PBM dimensions overflow RGBA buffer".to_string()))?;
    let mut pixels = Vec::with_capacity(capacity);

    for _ in 0..pixel_count {
        let bit = next_ascii_bit_token(bytes, &mut cursor)?
            .ok_or_else(|| ImageError::Decode("truncated P1 pixel data".to_string()))?;
        let gray = pbm_gray(bit)?;
        pixels.extend_from_slice(&[gray, gray, gray, 255]);
    }

    if next_ascii_bit_token(bytes, &mut cursor)?.is_some() {
        return Err(ImageError::Decode(
            "P1 contains more samples than its dimensions declare".to_string(),
        ));
    }

    Ok(RasterImage::new(width, height, pixels))
}

fn decode_p4(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    let mut cursor = 2; // `P4`
    let width = next_header_value(bytes, &mut cursor, "width")?;
    let height = next_header_value(bytes, &mut cursor, "height")?;
    validate_dimensions(width, height)?;
    cursor = binary_raster_start(bytes, cursor)?;

    let width_usize = width as usize;
    let height_usize = height as usize;
    let row_bytes = width_usize
        .checked_add(7)
        .ok_or_else(|| ImageError::Decode("PBM row width overflow".to_string()))?
        / 8;
    let expected = row_bytes
        .checked_mul(height_usize)
        .ok_or_else(|| ImageError::Decode("PBM dimensions overflow raster size".to_string()))?;
    let end = cursor
        .checked_add(expected)
        .ok_or_else(|| ImageError::Decode("PBM pixel offset overflow".to_string()))?;
    if bytes.len() < end {
        return Err(ImageError::Decode("truncated P4 pixel data".to_string()));
    }

    let pixel_count = checked_pixel_count(width, height)?;
    let capacity = pixel_count
        .checked_mul(4)
        .ok_or_else(|| ImageError::Decode("PBM dimensions overflow RGBA buffer".to_string()))?;
    let mut pixels = Vec::with_capacity(capacity);
    let raster = &bytes[cursor..end];

    for row in 0..height_usize {
        let row_data = &raster[row * row_bytes..(row + 1) * row_bytes];
        for column in 0..width_usize {
            let byte = row_data[column / 8];
            let bit = (byte >> (7 - (column % 8))) & 1;
            let gray = if bit == 1 { 0 } else { 255 };
            pixels.extend_from_slice(&[gray, gray, gray, 255]);
        }
    }

    Ok(RasterImage::new(width, height, pixels))
}

fn validate_dimensions(width: u32, height: u32) -> Result<(), ImageError> {
    if width == 0 || height == 0 {
        return Err(ImageError::Decode(
            "PBM width and height must be non-zero".to_string(),
        ));
    }
    Ok(())
}

fn checked_pixel_count(width: u32, height: u32) -> Result<usize, ImageError> {
    (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| ImageError::Decode("PBM dimensions overflow pixel count".to_string()))
}

fn pbm_gray(bit: u32) -> Result<u8, ImageError> {
    match bit {
        0 => Ok(255),
        1 => Ok(0),
        other => Err(ImageError::Decode(format!(
            "PBM sample must be 0 or 1, got {other}"
        ))),
    }
}

fn next_header_value(bytes: &[u8], cursor: &mut usize, field: &str) -> Result<u32, ImageError> {
    skip_header_space_and_comments(bytes, cursor);
    if *cursor >= bytes.len() {
        return Err(ImageError::Decode(format!("truncated P4 {field}")));
    }

    let start = *cursor;
    while *cursor < bytes.len() && bytes[*cursor].is_ascii_digit() {
        *cursor += 1;
    }
    if start == *cursor {
        return Err(ImageError::Decode("malformed P4 header".to_string()));
    }
    if *cursor < bytes.len() && !bytes[*cursor].is_ascii_whitespace() {
        return Err(ImageError::Decode("malformed P4 header".to_string()));
    }

    let text = std::str::from_utf8(&bytes[start..*cursor])
        .map_err(|_| ImageError::Decode("non-ASCII P4 header".to_string()))?;
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

fn binary_raster_start(bytes: &[u8], cursor: usize) -> Result<usize, ImageError> {
    if cursor >= bytes.len() || !bytes[cursor].is_ascii_whitespace() {
        return Err(ImageError::Decode(
            "P4 header must end with whitespace before pixel data".to_string(),
        ));
    }
    if bytes[cursor] == b'\r' && bytes.get(cursor + 1) == Some(&b'\n') {
        Ok(cursor + 2)
    } else {
        Ok(cursor + 1)
    }
}

/// Return the next unsigned decimal token, skipping Netpbm whitespace and
/// comments. Width/height and P1 raster bits share the same lexical grammar;
/// raster values themselves are constrained to 0/1 by `pbm_gray`.
fn next_ascii_bit_token(bytes: &[u8], cursor: &mut usize) -> Result<Option<u32>, ImageError> {
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
            "malformed P1 token at byte {start}"
        )));
    }
    if *cursor < bytes.len()
        && !bytes[*cursor].is_ascii_whitespace()
        && bytes[*cursor] != b'#'
    {
        return Err(ImageError::Decode(format!(
            "malformed P1 token at byte {start}"
        )));
    }

    let text = std::str::from_utf8(&bytes[start..*cursor])
        .map_err(|_| ImageError::Decode("non-ASCII P1 token".to_string()))?;
    let value = text
        .parse::<u32>()
        .map_err(|error| ImageError::Decode(error.to_string()))?;
    Ok(Some(value))
}

/// Decoded images keyed by resolved URL, including failures. This facade keeps
/// P1/P4 support on the same cache/fetch path used by `<img>` resources.
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
    fn p1_decodes_comments_and_inverts_bitmap_bits() {
        let image = decode(b"P1\n# pattern\n4 1\n0 1 # split\n1 0\n").expect("P1 image");
        assert_eq!((image.width, image.height), (4, 1));
        assert_eq!(image.pixel(0, 0), [255, 255, 255, 255]);
        assert_eq!(image.pixel(1, 0), [0, 0, 0, 255]);
        assert_eq!(image.pixel(2, 0), [0, 0, 0, 255]);
        assert_eq!(image.pixel(3, 0), [255, 255, 255, 255]);
    }

    #[test]
    fn p4_decodes_msb_first_rows_and_ignores_padding_bits() {
        let image = decode(b"P4\n10 1\n\xaa\x80").expect("P4 image");
        assert_eq!((image.width, image.height), (10, 1));
        let expected = [0u8, 255, 0, 255, 0, 255, 0, 255, 0, 255];
        for (x, gray) in expected.into_iter().enumerate() {
            assert_eq!(image.pixel(x as u32, 0), [gray, gray, gray, 255]);
        }
    }

    #[test]
    fn p4_row_padding_is_restarted_for_each_row() {
        let image = decode(b"P4\n3 2\n\xa0\x40").expect("two-row P4");
        assert_eq!(image.pixel(0, 0), [0, 0, 0, 255]);
        assert_eq!(image.pixel(1, 0), [255, 255, 255, 255]);
        assert_eq!(image.pixel(2, 0), [0, 0, 0, 255]);
        assert_eq!(image.pixel(0, 1), [255, 255, 255, 255]);
        assert_eq!(image.pixel(1, 1), [0, 0, 0, 255]);
        assert_eq!(image.pixel(2, 1), [255, 255, 255, 255]);
    }

    #[test]
    fn p4_crlf_separator_preserves_whitespace_valued_raster_byte() {
        let image = decode(b"P4\r\n8 1\r\n\x20").expect("CRLF P4");
        assert_eq!(image.pixel(0, 0), [255, 255, 255, 255]);
        assert_eq!(image.pixel(2, 0), [0, 0, 0, 255]);
    }

    #[test]
    fn pbm_rejects_invalid_bits_dimensions_and_truncation() {
        assert!(matches!(
            decode(b"P1 0 1 0"),
            Err(ImageError::Decode(message)) if message.contains("non-zero")
        ));
        assert!(matches!(
            decode(b"P1 1 1 2"),
            Err(ImageError::Decode(message)) if message.contains("0 or 1")
        ));
        assert!(matches!(
            decode(b"P1 1 1"),
            Err(ImageError::Decode(message)) if message.contains("truncated P1 pixel data")
        ));
        assert!(matches!(
            decode(b"P1 1 1 0 1"),
            Err(ImageError::Decode(message)) if message.contains("more samples")
        ));
        assert!(matches!(
            decode(b"P4\n9 1\n\xff"),
            Err(ImageError::Decode(message)) if message.contains("truncated P4 pixel data")
        ));
    }
}
