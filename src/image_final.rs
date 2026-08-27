// ============================================================
//  image_final.rs — public raster-image facade
// ============================================================
//
// Keep the existing image vocabulary and non-PNG decoders, but normalize PNG
// sample layouts before converting them to the painter's straight RGBA8 form.
// `png` 0.18 defaults to identity transformations, which leaves indexed,
// sub-8-bit grayscale, and 16-bit images in layouts the original decoder did
// not accept.

use std::collections::HashMap;
use std::rc::Rc;

use crate::net::{LoadError, ResourceLoader, Url};

pub use crate::image_base::{ImageError, ImageFormat, RasterImage};

/// Decode image bytes into the engine's straight RGBA8 representation.
pub fn decode(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    match ImageFormat::sniff(bytes) {
        Some(ImageFormat::Png) => decode_png_normalized(bytes),
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
}
