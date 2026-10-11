// ============================================================
//  image.rs  —  Raster images
// ============================================================
//
//  Decoded images are kept as straight RGBA8, which is what the painter
//  samples. PNG and JPEG are decoded by dedicated crates (the same division of
//  labour as glyph rasterization); PPM is handled here because the engine
//  writes that format itself.
//
//  `ImageCache` maps resolved URLs to decode results, so a page that uses the
//  same image twice fetches and decodes it once, and a broken URL is
//  remembered as broken rather than retried on every frame.

use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use crate::net::{LoadError, ResourceLoader, Url};

/// A decoded image: `width * height` RGBA8 pixels, top row first.
#[derive(Clone, PartialEq, Eq)]
pub struct RasterImage {
    pub width: u32,
    pub height: u32,
    /// RGBA, 4 bytes per pixel, not premultiplied.
    pub pixels: Vec<u8>,
}

impl fmt::Debug for RasterImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RasterImage({}x{})", self.width, self.height)
    }
}

impl RasterImage {
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        debug_assert_eq!(pixels.len(), (width as usize) * (height as usize) * 4);
        Self {
            width,
            height,
            pixels,
        }
    }

    /// Width divided by height; `None` for degenerate images.
    pub fn aspect_ratio(&self) -> Option<f32> {
        if self.width == 0 || self.height == 0 {
            None
        } else {
            Some(self.width as f32 / self.height as f32)
        }
    }

    /// The RGBA pixel at `(x, y)`, clamped to the image bounds.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        if self.width == 0 || self.height == 0 {
            return [0, 0, 0, 0];
        }
        let x = x.min(self.width - 1) as usize;
        let y = y.min(self.height - 1) as usize;
        let index = (y * self.width as usize + x) * 4;
        [
            self.pixels[index],
            self.pixels[index + 1],
            self.pixels[index + 2],
            self.pixels[index + 3],
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageError {
    /// The bytes did not match any format the engine can decode.
    UnsupportedFormat(String),
    /// The format was recognised but the data was broken.
    Decode(String),
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageError::UnsupportedFormat(what) => write!(f, "unsupported image format: {what}"),
            ImageError::Decode(message) => write!(f, "could not decode image: {message}"),
        }
    }
}

impl std::error::Error for ImageError {}

/// Image formats the engine recognises by signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    /// The binary PPM the painter itself writes.
    Ppm,
}

impl ImageFormat {
    /// Sniff the format from the leading bytes, as browsers do — the file
    /// extension and the `Content-Type` header are both often wrong.
    pub fn sniff(bytes: &[u8]) -> Option<ImageFormat> {
        if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
            Some(ImageFormat::Png)
        } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(ImageFormat::Jpeg)
        } else if bytes.starts_with(b"P6") {
            Some(ImageFormat::Ppm)
        } else {
            None
        }
    }
}

/// Decode image bytes into RGBA8.
pub fn decode(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    match ImageFormat::sniff(bytes) {
        Some(ImageFormat::Png) => decode_png(bytes),
        Some(ImageFormat::Jpeg) => decode_jpeg(bytes),
        Some(ImageFormat::Ppm) => decode_ppm(bytes),
        None => Err(ImageError::UnsupportedFormat(describe_prefix(bytes))),
    }
}

fn describe_prefix(bytes: &[u8]) -> String {
    let prefix: Vec<String> = bytes.iter().take(4).map(|b| format!("{b:02X}")).collect();
    if prefix.is_empty() {
        "empty file".to_string()
    } else {
        format!("starts with {}", prefix.join(" "))
    }
}

fn decode_png(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    // The decoders read from a `Read + Seek` source; a cursor over the bytes
    // avoids copying them.
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder
        .read_info()
        .map_err(|e| ImageError::Decode(e.to_string()))?;
    let mut buffer = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|e| ImageError::Decode(e.to_string()))?;

    let (width, height) = (info.width, info.height);
    let samples = &buffer[..info.buffer_size()];
    let pixels = match (info.color_type, info.bit_depth) {
        (png::ColorType::Rgba, png::BitDepth::Eight) => samples.to_vec(),
        (png::ColorType::Rgb, png::BitDepth::Eight) => {
            expand(samples, 3, |p| [p[0], p[1], p[2], 255])
        }
        (png::ColorType::Grayscale, png::BitDepth::Eight) => {
            expand(samples, 1, |p| [p[0], p[0], p[0], 255])
        }
        (png::ColorType::GrayscaleAlpha, png::BitDepth::Eight) => {
            expand(samples, 2, |p| [p[0], p[0], p[0], p[1]])
        }
        (color, depth) => {
            return Err(ImageError::Decode(format!(
                "unsupported PNG pixel layout: {color:?} at {depth:?}"
            )))
        }
    };

    Ok(RasterImage::new(width, height, pixels))
}

fn decode_jpeg(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    let mut decoder = jpeg_decoder::Decoder::new(std::io::Cursor::new(bytes));
    let samples = decoder
        .decode()
        .map_err(|e| ImageError::Decode(e.to_string()))?;
    let info = decoder
        .info()
        .ok_or_else(|| ImageError::Decode("missing JPEG frame header".into()))?;

    let pixels = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => expand(&samples, 3, |p| [p[0], p[1], p[2], 255]),
        jpeg_decoder::PixelFormat::L8 => expand(&samples, 1, |p| [p[0], p[0], p[0], 255]),
        jpeg_decoder::PixelFormat::L16 => {
            // 16-bit greyscale arrives little-endian; keep the high byte.
            expand(&samples, 2, |p| [p[1], p[1], p[1], 255])
        }
        jpeg_decoder::PixelFormat::CMYK32 => expand(&samples, 4, |p| {
            // Adobe JPEGs store CMYK inverted.
            let k = p[3] as u32;
            let to_rgb = |c: u8| ((c as u32 * k) / 255) as u8;
            [to_rgb(p[0]), to_rgb(p[1]), to_rgb(p[2]), 255]
        }),
    };

    Ok(RasterImage::new(
        info.width as u32,
        info.height as u32,
        pixels,
    ))
}

/// Expand packed samples into RGBA using `to_rgba` per source pixel.
fn expand(samples: &[u8], stride: usize, to_rgba: impl Fn(&[u8]) -> [u8; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() / stride * 4);
    for chunk in samples.chunks_exact(stride) {
        out.extend_from_slice(&to_rgba(chunk));
    }
    out
}

/// Decode binary PPM (P6) — the format the painter exports.
fn decode_ppm(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    let mut cursor = 2; // skip "P6"
    let mut fields = Vec::new();

    while fields.len() < 3 && cursor < bytes.len() {
        // Skip whitespace and `#` comments between header fields.
        while cursor < bytes.len() && (bytes[cursor] as char).is_whitespace() {
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
            return Err(ImageError::Decode("malformed PPM header".into()));
        }
        let text = std::str::from_utf8(&bytes[start..cursor]).unwrap_or("");
        fields.push(
            text.parse::<u32>()
                .map_err(|e| ImageError::Decode(e.to_string()))?,
        );
    }

    if fields.len() != 3 {
        return Err(ImageError::Decode("truncated PPM header".into()));
    }
    // Exactly one whitespace byte separates the header from the data.
    cursor += 1;

    let (width, height, max) = (fields[0], fields[1], fields[2]);
    if max != 255 {
        return Err(ImageError::Decode(format!(
            "unsupported PPM max value {max}"
        )));
    }
    let expected = (width as usize) * (height as usize) * 3;
    if bytes.len() < cursor + expected {
        return Err(ImageError::Decode("truncated PPM pixel data".into()));
    }

    let pixels = expand(&bytes[cursor..cursor + expected], 3, |p| {
        [p[0], p[1], p[2], 255]
    });
    Ok(RasterImage::new(width, height, pixels))
}

// ── Cache ─────────────────────────────────────────────────────────────────────

/// Decoded images keyed by resolved URL, including failures.
#[derive(Debug, Default, Clone)]
pub struct ImageCache {
    entries: HashMap<String, Result<Rc<RasterImage>, String>>,
}

impl ImageCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fetch and decode `url` unless it is already cached. Failures are cached
    /// too, so a missing image is not requested again on the next frame.
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

    /// Look up an already-fetched image.
    pub fn get(&self, url: &Url) -> Option<Rc<RasterImage>> {
        self.entries
            .get(&url.without_fragment().to_string())
            .and_then(|entry| entry.as_ref().ok().cloned())
    }

    /// The failure recorded for `url`, if the fetch or decode failed.
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

    /// Insert a decoded image directly (used by tests and embedders).
    pub fn insert(&mut self, url: &Url, image: RasterImage) {
        self.entries
            .insert(url.without_fragment().to_string(), Ok(Rc::new(image)));
    }

    /// Record a failed policy fetch or decode without bypassing the cache.
    ///
    /// Policy-aware document loaders do their network work above ImageCache,
    /// but broken images must retain the same negative-cache semantics as the
    /// legacy `fetch()` path so layout/paint and future refreshes do not retry
    /// a known failure forever.
    pub fn insert_error(&mut self, url: &Url, message: impl Into<String>) {
        self.entries
            .insert(url.without_fragment().to_string(), Err(message.into()));
    }
}

fn load_and_decode(url: &Url, loader: &dyn ResourceLoader) -> Result<Rc<RasterImage>, String> {
    let resource = loader.load(url).map_err(|e: LoadError| e.to_string())?;
    decode(&resource.bytes)
        .map(Rc::new)
        .map_err(|e| format!("{url}: {e}"))
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::MemoryLoader;

    /// A 2x2 PPM: red, green / blue, white.
    const PPM_2X2: &[u8] = b"P6\n2 2\n255\n\xff\x00\x00\x00\xff\x00\x00\x00\xff\xff\xff\xff";

    fn fixture(name: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples")
            .join("site")
            .join(name);
        std::fs::read(&path).unwrap_or_else(|e| panic!("missing fixture {}: {e}", path.display()))
    }

    #[test]
    fn sniffs_formats_from_signatures() {
        assert_eq!(ImageFormat::sniff(PPM_2X2), Some(ImageFormat::Ppm));
        assert_eq!(
            ImageFormat::sniff(&fixture("logo.png")),
            Some(ImageFormat::Png)
        );
        assert_eq!(
            ImageFormat::sniff(&fixture("photo.jpg")),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(ImageFormat::sniff(b"not an image"), None);
    }

    #[test]
    fn decodes_ppm_pixels() {
        let image = decode(PPM_2X2).expect("decoded");
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(image.pixel(1, 0), [0, 255, 0, 255]);
        assert_eq!(image.pixel(0, 1), [0, 0, 255, 255]);
        assert_eq!(image.pixel(1, 1), [255, 255, 255, 255]);
    }

    #[test]
    fn decodes_png_with_alpha() {
        let image = decode(&fixture("logo.png")).expect("decoded png");
        assert!(image.width > 0 && image.height > 0);
        assert_eq!(
            image.pixels.len(),
            (image.width * image.height * 4) as usize
        );
        // The fixture has a transparent margin, so some pixels must be see-through.
        assert!(
            image.pixels.chunks(4).any(|p| p[3] < 255),
            "expected alpha in logo.png"
        );
    }

    #[test]
    fn decodes_jpeg() {
        let image = decode(&fixture("photo.jpg")).expect("decoded jpeg");
        assert!(image.width > 0 && image.height > 0);
        assert_eq!(
            image.pixels.len(),
            (image.width * image.height * 4) as usize
        );
        // JPEG has no alpha channel.
        assert!(image.pixels.chunks(4).all(|p| p[3] == 255));
    }

    #[test]
    fn reports_unsupported_and_broken_data() {
        assert!(matches!(
            decode(b"hello"),
            Err(ImageError::UnsupportedFormat(_))
        ));
        assert!(matches!(
            decode(b"P6\n2 2\n255\n\xff"),
            Err(ImageError::Decode(_))
        ));
        let truncated_png = &fixture("logo.png")[..40];
        assert!(matches!(decode(truncated_png), Err(ImageError::Decode(_))));
    }

    #[test]
    fn aspect_ratio_is_width_over_height() {
        let image = RasterImage::new(4, 2, vec![0; 4 * 2 * 4]);
        assert_eq!(image.aspect_ratio(), Some(2.0));
        assert_eq!(RasterImage::new(0, 0, vec![]).aspect_ratio(), None);
    }

    #[test]
    fn cache_decodes_once_and_remembers_failures() {
        let mut loader = MemoryLoader::new();
        loader.insert("demo:///pic.ppm", PPM_2X2.to_vec());
        loader.insert("demo:///broken.png", b"not really a png".to_vec());

        let mut cache = ImageCache::new();
        let good = Url::parse("demo:///pic.ppm").unwrap();
        let first = cache.fetch(&good, &loader).expect("decoded");
        let second = cache.fetch(&good, &loader).expect("cached");
        assert!(
            Rc::ptr_eq(&first, &second),
            "second fetch should hit the cache"
        );

        let broken = Url::parse("demo:///broken.png").unwrap();
        assert!(cache.fetch(&broken, &loader).is_err());
        assert!(cache.error(&broken).is_some());
        assert!(cache.get(&broken).is_none());

        let missing = Url::parse("demo:///nope.png").unwrap();
        assert!(cache.fetch(&missing, &loader).is_err());
        assert_eq!(cache.len(), 3);
    }
}
