// ============================================================
//  image_pfm_final.rs — Netpbm PFM floating-point image facade
// ============================================================

use std::collections::HashMap;
use std::rc::Rc;

use crate::net::{LoadError, ResourceLoader, Url};

pub use crate::image_prev5::{ImageError, ImageFormat, RasterImage};

/// Decode image bytes into straight RGBA8, adding Netpbm-style PFM (`PF`/`Pf`)
/// support on top of PNG/JPEG/PNM/PAM.
pub fn decode(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    if bytes.starts_with(b"PF") || bytes.starts_with(b"Pf") {
        decode_pfm(bytes)
    } else {
        crate::image_prev5::decode(bytes)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Channels {
    Gray,
    Rgb,
}

impl Channels {
    fn count(self) -> usize {
        match self {
            Self::Gray => 1,
            Self::Rgb => 3,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PfmHeader {
    width: u32,
    height: u32,
    channels: Channels,
    little_endian: bool,
    scale: f32,
    raster_start: usize,
}

fn decode_pfm(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    let header = parse_header(bytes)?;
    let pixel_count = (header.width as usize)
        .checked_mul(header.height as usize)
        .ok_or_else(|| ImageError::Decode("PFM dimensions overflow pixel count".to_string()))?;
    let sample_count = pixel_count
        .checked_mul(header.channels.count())
        .ok_or_else(|| ImageError::Decode("PFM dimensions overflow sample count".to_string()))?;
    let raster_bytes = sample_count
        .checked_mul(4)
        .ok_or_else(|| ImageError::Decode("PFM dimensions overflow raster size".to_string()))?;
    let raster_end = header
        .raster_start
        .checked_add(raster_bytes)
        .ok_or_else(|| ImageError::Decode("PFM raster offset overflow".to_string()))?;
    if bytes.len() < raster_end {
        return Err(ImageError::Decode("truncated PFM raster".to_string()));
    }

    let capacity = pixel_count
        .checked_mul(4)
        .ok_or_else(|| ImageError::Decode("PFM dimensions overflow RGBA buffer".to_string()))?;
    let mut pixels = vec![0u8; capacity];
    let row_samples = (header.width as usize)
        .checked_mul(header.channels.count())
        .ok_or_else(|| ImageError::Decode("PFM row sample count overflow".to_string()))?;
    let row_bytes = row_samples
        .checked_mul(4)
        .ok_or_else(|| ImageError::Decode("PFM row size overflow".to_string()))?;
    let raster = &bytes[header.raster_start..raster_end];

    // Netpbm PFM stores rows bottom-to-top. RasterImage uses top-to-bottom.
    for file_y in 0..header.height as usize {
        let image_y = header.height as usize - 1 - file_y;
        let row = &raster[file_y * row_bytes..(file_y + 1) * row_bytes];
        for x in 0..header.width as usize {
            let sample_base = x * header.channels.count() * 4;
            let rgba = match header.channels {
                Channels::Gray => {
                    let gray = display_sample(read_sample(
                        &row[sample_base..sample_base + 4],
                        header.little_endian,
                    )?)?;
                    [gray, gray, gray, 255]
                }
                Channels::Rgb => [
                    display_sample(read_sample(
                        &row[sample_base..sample_base + 4],
                        header.little_endian,
                    )?)?,
                    display_sample(read_sample(
                        &row[sample_base + 4..sample_base + 8],
                        header.little_endian,
                    )?)?,
                    display_sample(read_sample(
                        &row[sample_base + 8..sample_base + 12],
                        header.little_endian,
                    )?)?,
                    255,
                ],
            };
            let out = (image_y * header.width as usize + x) * 4;
            pixels[out..out + 4].copy_from_slice(&rgba);
        }
    }

    // The magnitude is physical-units metadata in PFM rather than a generic
    // display transfer function. Keep it validated but do not multiply samples
    // by it when reducing the image to this engine's RGBA8 display surface.
    let _scale = header.scale;

    Ok(RasterImage::new(header.width, header.height, pixels))
}

fn parse_header(bytes: &[u8]) -> Result<PfmHeader, ImageError> {
    let (channels, mut cursor) = if bytes.starts_with(b"PF") {
        (Channels::Rgb, 2usize)
    } else if bytes.starts_with(b"Pf") {
        (Channels::Gray, 2usize)
    } else {
        return Err(ImageError::Decode("missing PFM PF/Pf signature".to_string()));
    };

    cursor = skip_header_whitespace(bytes, cursor, "PFM signature")?;
    let (width_text, next) = take_token(bytes, cursor, "PFM width")?;
    cursor = skip_header_whitespace(bytes, next, "PFM width")?;
    let (height_text, next) = take_token(bytes, cursor, "PFM height")?;
    cursor = skip_header_whitespace(bytes, next, "PFM height")?;
    let (scale_text, next) = take_token(bytes, cursor, "PFM scale")?;

    let width = parse_positive_u32(width_text, "width")?;
    let height = parse_positive_u32(height_text, "height")?;
    let scale_text = std::str::from_utf8(scale_text)
        .map_err(|_| ImageError::Decode("non-ASCII PFM scale".to_string()))?;
    let signed_scale = scale_text
        .parse::<f32>()
        .map_err(|_| ImageError::Decode("malformed PFM scale".to_string()))?;
    if !signed_scale.is_finite() || signed_scale == 0.0 {
        return Err(ImageError::Decode(
            "PFM scale must be finite and non-zero".to_string(),
        ));
    }
    let little_endian = signed_scale.is_sign_negative();
    let scale = signed_scale.abs();
    let raster_start = consume_raster_separator(bytes, next)?;

    Ok(PfmHeader {
        width,
        height,
        channels,
        little_endian,
        scale,
        raster_start,
    })
}

fn take_token<'a>(
    bytes: &'a [u8],
    start: usize,
    label: &str,
) -> Result<(&'a [u8], usize), ImageError> {
    if start >= bytes.len() || bytes[start].is_ascii_whitespace() {
        return Err(ImageError::Decode(format!("missing {label}")));
    }
    let mut end = start;
    while end < bytes.len() && !bytes[end].is_ascii_whitespace() {
        end += 1;
    }
    if end == start {
        return Err(ImageError::Decode(format!("missing {label}")));
    }
    Ok((&bytes[start..end], end))
}

fn skip_header_whitespace(
    bytes: &[u8],
    mut cursor: usize,
    label: &str,
) -> Result<usize, ImageError> {
    let start = cursor;
    while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    if cursor == start {
        return Err(ImageError::Decode(format!(
            "{label} must be followed by whitespace"
        )));
    }
    if cursor >= bytes.len() {
        return Err(ImageError::Decode("truncated PFM header".to_string()));
    }
    Ok(cursor)
}

fn consume_raster_separator(bytes: &[u8], cursor: usize) -> Result<usize, ImageError> {
    match bytes.get(cursor) {
        Some(b'\r') if bytes.get(cursor + 1) == Some(&b'\n') => Ok(cursor + 2),
        Some(byte) if byte.is_ascii_whitespace() => Ok(cursor + 1),
        _ => Err(ImageError::Decode(
            "PFM scale must be followed by whitespace".to_string(),
        )),
    }
}

fn parse_positive_u32(text: &[u8], field: &str) -> Result<u32, ImageError> {
    if text.is_empty() || !text.iter().all(u8::is_ascii_digit) {
        return Err(ImageError::Decode(format!("malformed PFM {field}")));
    }
    let text = std::str::from_utf8(text)
        .map_err(|_| ImageError::Decode(format!("non-ASCII PFM {field}")))?;
    let value = text
        .parse::<u32>()
        .map_err(|_| ImageError::Decode(format!("PFM {field} is out of range")))?;
    if value == 0 {
        return Err(ImageError::Decode(format!("PFM {field} must be positive")));
    }
    Ok(value)
}

fn read_sample(bytes: &[u8], little_endian: bool) -> Result<f32, ImageError> {
    let encoded: [u8; 4] = bytes
        .try_into()
        .map_err(|_| ImageError::Decode("truncated PFM sample".to_string()))?;
    let sample = if little_endian {
        f32::from_le_bytes(encoded)
    } else {
        f32::from_be_bytes(encoded)
    };
    if !sample.is_finite() {
        return Err(ImageError::Decode(
            "PFM raster contains a non-finite sample".to_string(),
        ));
    }
    Ok(sample)
}

fn display_sample(sample: f32) -> Result<u8, ImageError> {
    if !sample.is_finite() {
        return Err(ImageError::Decode(
            "PFM raster contains a non-finite sample".to_string(),
        ));
    }
    Ok((sample.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// Decoded images keyed by resolved URL, including failures. This facade keeps
/// PFM support on the same cache/fetch path used by `<img>` resources.
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

    fn push_le(out: &mut Vec<u8>, value: f32) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    fn push_be(out: &mut Vec<u8>, value: f32) {
        out.extend_from_slice(&value.to_be_bytes());
    }

    #[test]
    fn decodes_little_endian_rgb_and_flips_rows() {
        let mut bytes = b"PF\n1 2\n-1.0\n".to_vec();
        // File row 0 is the visual bottom.
        push_le(&mut bytes, 1.0);
        push_le(&mut bytes, 0.0);
        push_le(&mut bytes, 0.0);
        // File row 1 is the visual top.
        push_le(&mut bytes, 0.0);
        push_le(&mut bytes, 1.0);
        push_le(&mut bytes, 0.0);

        let image = decode(&bytes).expect("little-endian color PFM");
        assert_eq!(image.pixel(0, 0), [0, 255, 0, 255]);
        assert_eq!(image.pixel(0, 1), [255, 0, 0, 255]);
    }

    #[test]
    fn decodes_big_endian_grayscale() {
        let mut bytes = b"Pf\n2 1\n2.5\n".to_vec();
        push_be(&mut bytes, 0.25);
        push_be(&mut bytes, 0.75);
        let image = decode(&bytes).expect("big-endian grayscale PFM");
        assert_eq!(image.pixel(0, 0), [64, 64, 64, 255]);
        assert_eq!(image.pixel(1, 0), [191, 191, 191, 255]);
    }

    #[test]
    fn rejects_nonfinite_samples_and_zero_scale() {
        let mut bytes = b"Pf\n1 1\n-1\n".to_vec();
        push_le(&mut bytes, f32::NAN);
        assert!(decode(&bytes).is_err());
        assert!(decode(b"Pf\n1 1\n0\n\0\0\0\0").is_err());
    }

    #[test]
    fn rejects_truncated_and_overflowing_dimensions() {
        assert!(decode(b"Pf\n2 2\n-1\n\0\0\0\0").is_err());
        assert!(decode(b"PF\n4294967295 4294967295\n-1\n").is_err());
    }
}
