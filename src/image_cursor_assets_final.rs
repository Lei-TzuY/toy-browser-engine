// ============================================================
// image_cursor_assets_final.rs — generic CSS cursor asset facade
// ============================================================

use std::collections::HashMap;
use std::rc::Rc;

use crate::net::{LoadError, ResourceLoader, Url};

pub use crate::image_prev19::{
    decode, decode_cursor, CursorImage, ImageCache, ImageError, ImageFormat, RasterImage,
};

/// Decode an image for use as a CSS cursor.
///
/// Windows CUR containers keep their authored hotspot. Any other image format
/// supported by the normal image stack is accepted with the CSS default image
/// hotspot at the top-left corner `(0, 0)`.
pub fn decode_cursor_asset(bytes: &[u8]) -> Result<CursorImage, ImageError> {
    if looks_like_cur(bytes) {
        decode_cursor(bytes)
    } else {
        decode(bytes).map(|image| CursorImage {
            image,
            hotspot_x: 0,
            hotspot_y: 0,
        })
    }
}

fn looks_like_cur(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes[0..4] == [0, 0, 2, 0]
}

/// Cursor resources keyed by resolved URL, including failed loads/decodes.
///
/// Unlike the lower-level CUR-only cache, this CSS-facing cache accepts every
/// raster format the browser image stack can decode. CUR metadata is preserved;
/// ordinary images use a `(0, 0)` hotspot.
#[derive(Default, Clone)]
pub struct CursorCache {
    entries: HashMap<String, Result<Rc<CursorImage>, String>>,
}

impl CursorCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fetch(
        &mut self,
        url: &Url,
        loader: &dyn ResourceLoader,
    ) -> Result<Rc<CursorImage>, String> {
        let key = url.without_fragment().to_string();
        if let Some(entry) = self.entries.get(&key) {
            return entry.clone();
        }
        let outcome = load_and_decode_cursor_asset(url, loader);
        self.entries.insert(key, outcome.clone());
        outcome
    }

    pub fn get(&self, url: &Url) -> Option<Rc<CursorImage>> {
        self.entries
            .get(&url.without_fragment().to_string())
            .and_then(|entry| entry.as_ref().ok().cloned())
    }

    pub fn error(&self, url: &Url) -> Option<&str> {
        match self.entries.get(&url.without_fragment().to_string()) {
            Some(Err(error)) => Some(error),
            _ => None,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn insert(&mut self, url: &Url, cursor: CursorImage) {
        self.entries
            .insert(url.without_fragment().to_string(), Ok(Rc::new(cursor)));
    }
}

fn load_and_decode_cursor_asset(
    url: &Url,
    loader: &dyn ResourceLoader,
) -> Result<Rc<CursorImage>, String> {
    let resource = loader
        .load(url)
        .map_err(|error: LoadError| error.to_string())?;
    decode_cursor_asset(&resource.bytes)
        .map(Rc::new)
        .map_err(|error| format!("{url}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::MemoryLoader;

    fn png_rgba(rgba: [u8; 4]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&rgba).unwrap();
        }
        out
    }

    fn dib24(rgb: [u8; 3]) -> Vec<u8> {
        let mut out = vec![0u8; 48];
        out[0..4].copy_from_slice(&40u32.to_le_bytes());
        out[4..8].copy_from_slice(&1i32.to_le_bytes());
        out[8..12].copy_from_slice(&2i32.to_le_bytes());
        out[12..14].copy_from_slice(&1u16.to_le_bytes());
        out[14..16].copy_from_slice(&24u16.to_le_bytes());
        out[40..43].copy_from_slice(&[rgb[2], rgb[1], rgb[0]]);
        out
    }

    fn cur(hotspot: (u16, u16), rgb: [u8; 3]) -> Vec<u8> {
        let payload = dib24(rgb);
        let mut out = vec![0u8; 22];
        out[2..4].copy_from_slice(&2u16.to_le_bytes());
        out[4..6].copy_from_slice(&1u16.to_le_bytes());
        out[6] = 1;
        out[7] = 1;
        out[10..12].copy_from_slice(&hotspot.0.to_le_bytes());
        out[12..14].copy_from_slice(&hotspot.1.to_le_bytes());
        out[14..18].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        out[18..22].copy_from_slice(&22u32.to_le_bytes());
        out.extend_from_slice(&payload);
        out
    }

    #[test]
    fn generic_png_cursor_defaults_to_top_left_hotspot() {
        let cursor = decode_cursor_asset(&png_rgba([10, 20, 30, 40])).unwrap();
        assert_eq!(cursor.hotspot(), (0, 0));
        assert_eq!(cursor.image.pixel(0, 0), [10, 20, 30, 40]);
    }

    #[test]
    fn native_cur_keeps_container_hotspot() {
        let cursor = decode_cursor_asset(&cur((0, 0), [4, 5, 6])).unwrap();
        assert_eq!(cursor.hotspot(), (0, 0));
        assert_eq!(cursor.image.pixel(0, 0), [4, 5, 6, 255]);
    }

    #[test]
    fn cursor_cache_accepts_generic_images_and_remembers_failures() {
        let mut loader = MemoryLoader::new();
        loader.insert("demo:///pointer.png", png_rgba([1, 2, 3, 200]));
        loader.insert("demo:///broken.png", b"not an image".to_vec());
        let png = Url::parse("demo:///pointer.png").unwrap();
        let broken = Url::parse("demo:///broken.png").unwrap();
        let mut cache = CursorCache::new();

        let cursor = cache.fetch(&png, &loader).unwrap();
        assert_eq!(cursor.hotspot(), (0, 0));
        assert_eq!(cursor.image.pixel(0, 0), [1, 2, 3, 200]);
        assert!(cache.fetch(&broken, &loader).is_err());
        assert!(cache.error(&broken).is_some());
        assert_eq!(cache.len(), 2);
    }
}
