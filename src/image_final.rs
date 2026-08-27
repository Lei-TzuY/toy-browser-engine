// ============================================================
//  image_final.rs — public raster-image facade
// ============================================================
//
// Keep the existing image vocabulary and JPEG decoder, normalize PNG sample
// layouts before converting them to the painter's straight RGBA8 form, and
// decode PPM with checked dimensions and the full P6 sample-depth range.

use std::collections::HashMap;
use std::rc::Rc;

use crate::net::{LoadError, ResourceLoader, Url};

pub use crate::image_base::{ImageError, ImageFormat, RasterImage};

/// Decode image bytes into the engine's straight RGBA8 representation.
pub fn decode(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    match ImageFormat::sniff(bytes) {
        Some(ImageFormat::Png) => decode_png_normalized(bytes),
        Some(ImageFormat::Ppm) => decode_ppm_checked(bytes),
        _ => crate::image_base::decode(bytes),
    }
}

fn decode_png_normalized(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder
        .read_info()
        .map_err(|error| ImageError::Decode(error.to_string()))?;
    let output_size = reader.output_buffer_size().ok_or_else(|| {
        ImageError::Decode("PNG output buffer exceeds decoder limits".to_string())
    })?;
    let mut buffer = vec![0; output_size];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|error| ImageError::Decode(error.to_string()))?;

    let samples = &buffer[..info.buffer_size()];
    let pixels = match (info.color_type, info.bit_depth) {
        (png::ColorType::Rgba, png::BitDepth::Eight) => samples.to_vec(),
        (png::ColorType::Rgb, png::BitDepth::Eight) => {
            expand(samples, 3, |pixel| [pixel[0], pixel[1], pixel[2], 255])
        }
        (png::ColorType::Grayscale, png::BitDepth::Eight) => {
            expand(samples, 1, |pixel| [pixel[0], pixel[0], pixel[0], 255])
        }
        (png::ColorType::GrayscaleAlpha, png::BitDepth::Eight) => expand(samples, 2, |pixel| {
            [pixel[0], pixel[0], pixel[0], pixel[1]]
        }),
        (color, depth) => {
            return Err(ImageError::Decode(format!(
                "PNG normalization produced unsupported layout: {color:?} at {depth:?}"
            )))
        }
    };

    Ok(RasterImage::new(info.width, info.height, pixels))
}

fn decode_ppm_checked(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    let mut cursor = 2; // signature `P6` was already sniffed.
    let mut fields = Vec::with_capacity(3);

    while fields.len() < 3 && cursor < bytes.len() {
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor < bytes.len() && bytes[cursor] == b'#' {
            while cursor < bytes.len() && bytes[cursor] != b'\n' {
                cursor += 1;
            }
            continue;
        }

        let start = cursor;
        while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
            cursor += 1;
        }
        if start == cursor {
            return Err(ImageError::Decode("malformed PPM header".to_string()));
        }
        let text = std::str::from_utf8(&bytes[start..cursor])
            .map_err(|_| ImageError::Decode("non-ASCII PPM header".to_string()))?;
        let value = text
            .parse::<u32>()
            .map_err(|error| ImageError::Decode(error.to_string()))?;
        fields.push(value);
    }

    if fields.len() != 3 {
        return Err(ImageError::Decode("truncated PPM header".to_string()));
    }
    if cursor >= bytes.len() || !bytes[cursor].is_ascii_whitespace() {
        return Err(ImageError::Decode(
            "PPM header must end with whitespace before pixel data".to_string(),
        ));
    }
    // Netpbm files commonly use CRLF line endings. Treat that pair as the one
    // raster separator without consuming arbitrary additional whitespace,
    // because a pixel sample is allowed to have a whitespace byte value.
    if bytes[cursor] == b'\r' && bytes.get(cursor + 1) == Some(&b'\n') {
        cursor += 2;
    } else {
        cursor += 1;
    }

    let (width, height, max) = (fields[0], fields[1], fields[2]);
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

    let sample_bytes = if max < 256 { 1usize } else { 2usize };
    let sample_count = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(3))
        .ok_or_else(|| ImageError::Decode("PPM dimensions overflow sample count".to_string()))?;
    let expected = sample_count
        .checked_mul(sample_bytes)
        .ok_or_else(|| ImageError::Decode("PPM dimensions overflow pixel buffer".to_string()))?;
    let end = cursor
        .checked_add(expected)
        .ok_or_else(|| ImageError::Decode("PPM pixel offset overflow".to_string()))?;
    if bytes.len() < end {
        return Err(ImageError::Decode("truncated PPM pixel data".to_string()));
    }

    let pixel_count = sample_count / 3;
    let capacity = pixel_count
        .checked_mul(4)
        .ok_or_else(|| ImageError::Decode("PPM dimensions overflow RGBA buffer".to_string()))?;
    let mut pixels = Vec::with_capacity(capacity);
    let raster = &bytes[cursor..end];

    if sample_bytes == 1 {
        for rgb in raster.chunks_exact(3) {
            let r = scale_ppm_sample(rgb[0] as u32, max)?;
            let g = scale_ppm_sample(rgb[1] as u32, max)?;
            let b = scale_ppm_sample(rgb[2] as u32, max)?;
            pixels.extend_from_slice(&[r, g, b, 255]);
        }
    } else {
        for rgb in raster.chunks_exact(6) {
            let r = u16::from_be_bytes([rgb[0], rgb[1]]) as u32;
            let g = u16::from_be_bytes([rgb[2], rgb[3]]) as u32;
            let b = u16::from_be_bytes([rgb[4], rgb[5]]) as u32;
            pixels.extend_from_slice(&[
                scale_ppm_sample(r, max)?,
                scale_ppm_sample(g, max)?,
                scale_ppm_sample(b, max)?,
                255,
            ]);
        }
    }

    Ok(RasterImage::new(width, height, pixels))
}

fn scale_ppm_sample(sample: u32, max: u32) -> Result<u8, ImageError> {
    if sample > max {
        return Err(ImageError::Decode(format!(
            "PPM sample {sample} exceeds max value {max}"
        )));
    }
    // Round to the nearest RGBA8 value rather than truncating darker.
    Ok(((sample * 255 + max / 2) / max) as u8)
}

fn expand(samples: &[u8], stride: usize, to_rgba: impl Fn(&[u8]) -> [u8; 4]) -> Vec<u8> {
    let mut output = Vec::with_capacity(samples.len() / stride * 4);
    for sample in samples.chunks_exact(stride) {
        output.extend_from_slice(&to_rgba(sample));
    }
    output
}

/// Decoded images keyed by resolved URL, including failures.
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
    use crate::net::MemoryLoader;

    const PPM_1X1: &[u8] = b"P6\n1 1\n255\n\x12\x34\x56";

    fn indexed_png() -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
            encoder.set_color(png::ColorType::Indexed);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_palette(vec![255, 0, 0, 0, 255, 0]);
            encoder.set_trns(vec![0, 255]);
            let mut writer = encoder.write_header().expect("indexed PNG header");
            writer
                .write_image_data(&[0, 1])
                .expect("indexed PNG pixels");
        }
        bytes
    }

    fn rgb16_png() -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Sixteen);
            let mut writer = encoder.write_header().expect("16-bit PNG header");
            writer
                .write_image_data(&[0x12, 0x34, 0xAB, 0xCD, 0xFE, 0xDC])
                .expect("16-bit PNG pixel");
        }
        bytes
    }

    #[test]
    fn expands_indexed_png_palette_and_transparency() {
        let image = decode(&indexed_png()).expect("decoded indexed PNG");
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.pixel(0, 0), [255, 0, 0, 0]);
        assert_eq!(image.pixel(1, 0), [0, 255, 0, 255]);
    }

    #[test]
    fn strips_16_bit_png_samples_to_rgba8() {
        let image = decode(&rgb16_png()).expect("decoded 16-bit PNG");
        assert_eq!((image.width, image.height), (1, 1));
        assert_eq!(image.pixel(0, 0), [0x12, 0xAB, 0xFE, 255]);
    }

    #[test]
    fn cache_uses_the_normalized_png_decoder() {
        let mut loader = MemoryLoader::new();
        loader.insert("demo:///indexed.png", indexed_png());
        let url = Url::parse("demo:///indexed.png").unwrap();
        let mut cache = ImageCache::new();

        let image = cache.fetch(&url, &loader).expect("cached indexed PNG");
        assert_eq!(image.pixel(0, 0), [255, 0, 0, 0]);
        assert_eq!(image.pixel(1, 0), [0, 255, 0, 255]);
        assert!(Rc::ptr_eq(
            &image,
            &cache.fetch(&url, &loader).expect("cache hit")
        ));
    }

    #[test]
    fn checked_ppm_decoder_preserves_normal_pixels() {
        let image = decode(PPM_1X1).expect("decoded PPM");
        assert_eq!((image.width, image.height), (1, 1));
        assert_eq!(image.pixel(0, 0), [0x12, 0x34, 0x56, 255]);
    }

    #[test]
    fn ppm_scales_low_maxval_samples_to_rgba8() {
        let image = decode(b"P6\n1 1\n100\n\x00\x32\x64").expect("low-max PPM");
        assert_eq!(image.pixel(0, 0), [0, 128, 255, 255]);
    }

    #[test]
    fn ppm_decodes_16_bit_big_endian_samples() {
        let image = decode(
            b"P6\n1 1\n65535\n\x00\x00\x80\x00\xff\xff",
        )
        .expect("16-bit PPM");
        assert_eq!(image.pixel(0, 0), [0, 128, 255, 255]);
    }

    #[test]
    fn ppm_accepts_crlf_raster_separator_without_consuming_pixels() {
        let image = decode(b"P6\r\n1 1\r\n255\r\n\x0a\x20\xff").expect("CRLF PPM");
        assert_eq!(image.pixel(0, 0), [10, 32, 255, 255]);
    }

    #[test]
    fn ppm_rejects_sample_above_declared_maxval() {
        assert!(matches!(
            decode(b"P6\n1 1\n100\n\x65\x00\x00"),
            Err(ImageError::Decode(message)) if message.contains("exceeds max value")
        ));
    }

    #[test]
    fn ppm_rejects_invalid_maxval_range() {
        assert!(matches!(
            decode(b"P6\n1 1\n0\n"),
            Err(ImageError::Decode(message)) if message.contains("1..=65535")
        ));
        assert!(matches!(
            decode(b"P6\n1 1\n65536\n"),
            Err(ImageError::Decode(message)) if message.contains("1..=65535")
        ));
    }

    #[test]
    fn ppm_rejects_zero_dimensions() {
        assert!(matches!(
            decode(b"P6\n0 1\n255\n"),
            Err(ImageError::Decode(message)) if message.contains("non-zero")
        ));
    }

    #[test]
    fn ppm_rejects_dimension_arithmetic_overflow() {
        assert!(matches!(
            decode(b"P6\n4294967295 4294967295\n255\n"),
            Err(ImageError::Decode(message)) if message.contains("overflow")
        ));
    }
}
