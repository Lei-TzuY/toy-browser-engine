// ============================================================
//  image_pam_final.rs — Netpbm PAM visual-image extension facade
// ============================================================

use std::collections::HashMap;
use std::rc::Rc;

use crate::net::{LoadError, ResourceLoader, Url};

pub use crate::image_prev4::{ImageError, ImageFormat, RasterImage};

/// Decode image bytes into straight RGBA8, adding the standard visual PAM/P7
/// tuple types on top of PNG/JPEG/PPM/PGM/PBM support.
pub fn decode(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    if bytes.starts_with(b"P7") {
        decode_pam(bytes)
    } else {
        crate::image_prev4::decode(bytes)
    }
}

#[derive(Debug)]
struct PamHeader {
    width: u32,
    height: u32,
    depth: u32,
    maxval: u32,
    tuple_type: String,
    raster_start: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisualTuple {
    BlackAndWhite,
    BlackAndWhiteAlpha,
    Grayscale,
    GrayscaleAlpha,
    Rgb,
    RgbAlpha,
}

impl VisualTuple {
    fn parse(tuple_type: &str) -> Result<Self, ImageError> {
        match tuple_type {
            "BLACKANDWHITE" => Ok(Self::BlackAndWhite),
            "BLACKANDWHITE_ALPHA" => Ok(Self::BlackAndWhiteAlpha),
            "GRAYSCALE" => Ok(Self::Grayscale),
            "GRAYSCALE_ALPHA" => Ok(Self::GrayscaleAlpha),
            "RGB" => Ok(Self::Rgb),
            "RGB_ALPHA" => Ok(Self::RgbAlpha),
            "" => Err(ImageError::Decode(
                "PAM visual decoding requires TUPLTYPE".to_string(),
            )),
            other => Err(ImageError::Decode(format!(
                "unsupported PAM tuple type {other:?}"
            ))),
        }
    }

    fn minimum_depth(self) -> u32 {
        match self {
            Self::BlackAndWhite | Self::Grayscale => 1,
            Self::BlackAndWhiteAlpha | Self::GrayscaleAlpha => 2,
            Self::Rgb => 3,
            Self::RgbAlpha => 4,
        }
    }

    fn validate(self, header: &PamHeader) -> Result<(), ImageError> {
        if header.depth < self.minimum_depth() {
            return Err(ImageError::Decode(format!(
                "PAM tuple type {} requires depth at least {}, got {}",
                header.tuple_type,
                self.minimum_depth(),
                header.depth
            )));
        }
        if matches!(self, Self::BlackAndWhite | Self::BlackAndWhiteAlpha) && header.maxval != 1 {
            return Err(ImageError::Decode(format!(
                "PAM {} requires maxval 1, got {}",
                header.tuple_type, header.maxval
            )));
        }
        Ok(())
    }
}

fn decode_pam(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    let header = parse_header(bytes)?;
    let tuple = VisualTuple::parse(&header.tuple_type)?;
    tuple.validate(&header)?;

    let sample_bytes = if header.maxval < 256 { 1usize } else { 2usize };
    let pixel_count = (header.width as usize)
        .checked_mul(header.height as usize)
        .ok_or_else(|| ImageError::Decode("PAM dimensions overflow pixel count".to_string()))?;
    let sample_count = pixel_count
        .checked_mul(header.depth as usize)
        .ok_or_else(|| ImageError::Decode("PAM dimensions overflow sample count".to_string()))?;
    let raster_bytes = sample_count
        .checked_mul(sample_bytes)
        .ok_or_else(|| ImageError::Decode("PAM dimensions overflow raster size".to_string()))?;
    let raster_end = header
        .raster_start
        .checked_add(raster_bytes)
        .ok_or_else(|| ImageError::Decode("PAM raster offset overflow".to_string()))?;
    if bytes.len() < raster_end {
        return Err(ImageError::Decode("truncated PAM raster".to_string()));
    }

    let capacity = pixel_count
        .checked_mul(4)
        .ok_or_else(|| ImageError::Decode("PAM dimensions overflow RGBA buffer".to_string()))?;
    let mut pixels = Vec::with_capacity(capacity);
    let raster = &bytes[header.raster_start..raster_end];
    let tuple_stride = (header.depth as usize)
        .checked_mul(sample_bytes)
        .ok_or_else(|| ImageError::Decode("PAM tuple stride overflow".to_string()))?;

    for encoded_tuple in raster.chunks_exact(tuple_stride) {
        let sample = |plane: usize| -> Result<u32, ImageError> {
            let offset = plane * sample_bytes;
            let value = if sample_bytes == 1 {
                encoded_tuple[offset] as u32
            } else {
                u16::from_be_bytes([encoded_tuple[offset], encoded_tuple[offset + 1]]) as u32
            };
            if value > header.maxval {
                return Err(ImageError::Decode(format!(
                    "PAM sample {value} exceeds max value {}",
                    header.maxval
                )));
            }
            Ok(value)
        };

        let rgba = match tuple {
            VisualTuple::BlackAndWhite => {
                let gray = scale_sample(sample(0)?, header.maxval)?;
                [gray, gray, gray, 255]
            }
            VisualTuple::BlackAndWhiteAlpha => {
                let gray = scale_sample(sample(0)?, header.maxval)?;
                let alpha = scale_sample(sample(1)?, header.maxval)?;
                [gray, gray, gray, alpha]
            }
            VisualTuple::Grayscale => {
                let gray = scale_sample(sample(0)?, header.maxval)?;
                [gray, gray, gray, 255]
            }
            VisualTuple::GrayscaleAlpha => {
                let gray = scale_sample(sample(0)?, header.maxval)?;
                let alpha = scale_sample(sample(1)?, header.maxval)?;
                [gray, gray, gray, alpha]
            }
            VisualTuple::Rgb => [
                scale_sample(sample(0)?, header.maxval)?,
                scale_sample(sample(1)?, header.maxval)?,
                scale_sample(sample(2)?, header.maxval)?,
                255,
            ],
            VisualTuple::RgbAlpha => [
                scale_sample(sample(0)?, header.maxval)?,
                scale_sample(sample(1)?, header.maxval)?,
                scale_sample(sample(2)?, header.maxval)?,
                scale_sample(sample(3)?, header.maxval)?,
            ],
        };
        pixels.extend_from_slice(&rgba);
    }

    Ok(RasterImage::new(header.width, header.height, pixels))
}

fn parse_header(bytes: &[u8]) -> Result<PamHeader, ImageError> {
    if !bytes.starts_with(b"P7") {
        return Err(ImageError::Decode("missing PAM P7 signature".to_string()));
    }
    let mut cursor = 2usize;
    cursor = consume_line_ending(bytes, cursor)
        .ok_or_else(|| ImageError::Decode("PAM P7 signature must end with a newline".to_string()))?;

    let mut width = None;
    let mut height = None;
    let mut depth = None;
    let mut maxval = None;
    let mut tuple_parts = Vec::new();

    loop {
        if cursor >= bytes.len() {
            return Err(ImageError::Decode("truncated PAM header".to_string()));
        }
        let line_start = cursor;
        while cursor < bytes.len() && bytes[cursor] != b'\n' && bytes[cursor] != b'\r' {
            cursor += 1;
        }
        if cursor >= bytes.len() {
            return Err(ImageError::Decode("PAM header line missing newline".to_string()));
        }
        let line_end = cursor;
        cursor = consume_line_ending(bytes, cursor)
            .ok_or_else(|| ImageError::Decode("PAM header line missing newline".to_string()))?;

        let line = std::str::from_utf8(&bytes[line_start..line_end])
            .map_err(|_| ImageError::Decode("non-ASCII PAM header".to_string()))?;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed == "ENDHDR" {
            break;
        }

        let (name, rest) = split_header_field(trimmed)?;
        match name {
            "WIDTH" => set_once(&mut width, parse_decimal(rest, "WIDTH")?, "WIDTH")?,
            "HEIGHT" => set_once(&mut height, parse_decimal(rest, "HEIGHT")?, "HEIGHT")?,
            "DEPTH" => set_once(&mut depth, parse_decimal(rest, "DEPTH")?, "DEPTH")?,
            "MAXVAL" => set_once(&mut maxval, parse_decimal(rest, "MAXVAL")?, "MAXVAL")?,
            "TUPLTYPE" => {
                let value = rest.trim();
                if value.is_empty() {
                    return Err(ImageError::Decode(
                        "PAM TUPLTYPE requires a value".to_string(),
                    ));
                }
                tuple_parts.push(value.to_string());
            }
            other => {
                return Err(ImageError::Decode(format!(
                    "unsupported PAM header field {other:?}"
                )))
            }
        }
    }

    let width = width.ok_or_else(|| ImageError::Decode("missing PAM WIDTH".to_string()))?;
    let height = height.ok_or_else(|| ImageError::Decode("missing PAM HEIGHT".to_string()))?;
    let depth = depth.ok_or_else(|| ImageError::Decode("missing PAM DEPTH".to_string()))?;
    let maxval = maxval.ok_or_else(|| ImageError::Decode("missing PAM MAXVAL".to_string()))?;

    if width == 0 || height == 0 || depth == 0 {
        return Err(ImageError::Decode(
            "PAM width, height and depth must be non-zero".to_string(),
        ));
    }
    if !(1..=65_535).contains(&maxval) {
        return Err(ImageError::Decode(format!(
            "PAM max value must be in 1..=65535, got {maxval}"
        )));
    }

    Ok(PamHeader {
        width,
        height,
        depth,
        maxval,
        tuple_type: tuple_parts.join(" "),
        raster_start: cursor,
    })
}

fn consume_line_ending(bytes: &[u8], cursor: usize) -> Option<usize> {
    match bytes.get(cursor) {
        Some(b'\n') => Some(cursor + 1),
        Some(b'\r') if bytes.get(cursor + 1) == Some(&b'\n') => Some(cursor + 2),
        _ => None,
    }
}

fn split_header_field(line: &str) -> Result<(&str, &str), ImageError> {
    let mut split = line.splitn(2, char::is_whitespace);
    let name = split.next().unwrap_or_default();
    if name.len() > 8 {
        return Err(ImageError::Decode(format!(
            "PAM header field name too long: {name:?}"
        )));
    }
    let rest = split.next().unwrap_or_default().trim();
    if rest.is_empty() {
        return Err(ImageError::Decode(format!(
            "PAM header field {name} requires a value"
        )));
    }
    Ok((name, rest))
}

fn parse_decimal(text: &str, field: &str) -> Result<u32, ImageError> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ImageError::Decode(format!(
            "malformed PAM {field} value"
        )));
    }
    text.parse::<u32>()
        .map_err(|error| ImageError::Decode(error.to_string()))
}

fn set_once(slot: &mut Option<u32>, value: u32, field: &str) -> Result<(), ImageError> {
    if slot.replace(value).is_some() {
        return Err(ImageError::Decode(format!(
            "duplicate PAM {field} header"
        )));
    }
    Ok(())
}

fn scale_sample(sample: u32, maxval: u32) -> Result<u8, ImageError> {
    if sample > maxval {
        return Err(ImageError::Decode(format!(
            "PAM sample {sample} exceeds max value {maxval}"
        )));
    }
    Ok(((sample * 255 + maxval / 2) / maxval) as u8)
}

/// Decoded images keyed by resolved URL, including failures. This facade keeps
/// PAM support on the same cache/fetch path used by `<img>` resources.
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
    fn decodes_rgb_alpha_and_scales_samples() {
        let image = decode(b"P7\nWIDTH 1\nHEIGHT 1\nDEPTH 4\nMAXVAL 100\nTUPLTYPE RGB_ALPHA\nENDHDR\n\x00\x32\x64\x19")
            .expect("RGB_ALPHA PAM");
        assert_eq!(image.pixel(0, 0), [0, 128, 255, 64]);
    }

    #[test]
    fn decodes_sixteen_bit_grayscale_alpha() {
        let image = decode(b"P7\nWIDTH 1\nHEIGHT 1\nDEPTH 2\nMAXVAL 65535\nTUPLTYPE GRAYSCALE_ALPHA\nENDHDR\n\x80\x00\xff\xff")
            .expect("16-bit grayscale-alpha PAM");
        assert_eq!(image.pixel(0, 0), [128, 128, 128, 255]);
    }

    #[test]
    fn black_and_white_uses_pam_not_pbm_polarity() {
        let image = decode(b"P7\nWIDTH 2\nHEIGHT 1\nDEPTH 1\nMAXVAL 1\nTUPLTYPE BLACKANDWHITE\nENDHDR\n\x00\x01")
            .expect("black-and-white PAM");
        assert_eq!(image.pixel(0, 0), [0, 0, 0, 255]);
        assert_eq!(image.pixel(1, 0), [255, 255, 255, 255]);
    }

    #[test]
    fn accepts_extra_planes_for_defined_visual_tuple() {
        let image = decode(b"P7\nWIDTH 1\nHEIGHT 1\nDEPTH 5\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n\x01\x02\x03\x04\xff")
            .expect("extra-plane PAM");
        assert_eq!(image.pixel(0, 0), [1, 2, 3, 4]);
    }

    #[test]
    fn rejects_unknown_tuple_missing_fields_and_bad_depth() {
        assert!(matches!(
            decode(b"P7\nWIDTH 1\nHEIGHT 1\nDEPTH 3\nMAXVAL 255\nTUPLTYPE CUSTOM\nENDHDR\n\0\0\0"),
            Err(ImageError::Decode(message)) if message.contains("unsupported PAM tuple type")
        ));
        assert!(matches!(
            decode(b"P7\nWIDTH 1\nHEIGHT 1\nDEPTH 3\nTUPLTYPE RGB\nENDHDR\n"),
            Err(ImageError::Decode(message)) if message.contains("missing PAM MAXVAL")
        ));
        assert!(matches!(
            decode(b"P7\nWIDTH 1\nHEIGHT 1\nDEPTH 2\nMAXVAL 255\nTUPLTYPE RGB\nENDHDR\n\0\0"),
            Err(ImageError::Decode(message)) if message.contains("requires depth")
        ));
    }

    #[test]
    fn rejects_duplicate_fields_truncated_raster_and_out_of_range_sample() {
        assert!(matches!(
            decode(b"P7\nWIDTH 1\nWIDTH 2\nHEIGHT 1\nDEPTH 1\nMAXVAL 255\nTUPLTYPE GRAYSCALE\nENDHDR\n\0"),
            Err(ImageError::Decode(message)) if message.contains("duplicate PAM WIDTH")
        ));
        assert!(matches!(
            decode(b"P7\nWIDTH 2\nHEIGHT 1\nDEPTH 1\nMAXVAL 255\nTUPLTYPE GRAYSCALE\nENDHDR\n\0"),
            Err(ImageError::Decode(message)) if message.contains("truncated PAM raster")
        ));
        assert!(matches!(
            decode(b"P7\nWIDTH 1\nHEIGHT 1\nDEPTH 1\nMAXVAL 100\nTUPLTYPE GRAYSCALE\nENDHDR\n\x65"),
            Err(ImageError::Decode(message)) if message.contains("exceeds max value")
        ));
    }
}
