// ============================================================
// image_ico_final.rs — Windows ICO favicon container facade
// ============================================================

use std::collections::HashMap;
use std::rc::Rc;

use crate::net::{LoadError, ResourceLoader, Url};

pub use crate::image::{ImageError, ImageFormat, RasterImage};

/// Decode image bytes into straight RGBA8, adding Windows ICO container support.
///
/// Modern ICO files may store PNG-compressed icon images. This facade validates
/// the icon directory, deterministically chooses the largest advertised image
/// (breaking ties by bit depth and then directory order), bounds-checks the
/// selected payload, and delegates PNG decoding to the existing image stack.
pub fn decode(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    if looks_like_ico(bytes) {
        decode_ico(bytes)
    } else {
        crate::image::decode(bytes)
    }
}

fn looks_like_ico(bytes: &[u8]) -> bool {
    bytes.len() >= 6 && bytes[0..4] == [0, 0, 1, 0]
}

fn read_u16(bytes: &[u8], offset: usize, field: &str) -> Result<u16, ImageError> {
    let raw = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| ImageError::Decode(format!("truncated ICO {field}")))?;
    Ok(u16::from_le_bytes(raw.try_into().unwrap()))
}

fn read_u32(bytes: &[u8], offset: usize, field: &str) -> Result<u32, ImageError> {
    let raw = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| ImageError::Decode(format!("truncated ICO {field}")))?;
    Ok(u32::from_le_bytes(raw.try_into().unwrap()))
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    width: u32,
    height: u32,
    bit_depth: u16,
    size: usize,
    offset: usize,
    ordinal: usize,
}

fn decode_ico(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    if bytes.len() < 6 {
        return Err(ImageError::Decode("truncated ICO header".into()));
    }
    if read_u16(bytes, 0, "reserved field")? != 0 {
        return Err(ImageError::Decode("ICO reserved field must be zero".into()));
    }
    if read_u16(bytes, 2, "type")? != 1 {
        return Err(ImageError::Decode("ICO type must be 1 (icon)".into()));
    }

    let count = read_u16(bytes, 4, "image count")? as usize;
    if count == 0 {
        return Err(ImageError::Decode("ICO must contain at least one image".into()));
    }
    let directory_end = 6usize
        .checked_add(count.checked_mul(16).ok_or_else(|| ImageError::Decode("ICO directory size overflow".into()))?)
        .ok_or_else(|| ImageError::Decode("ICO directory size overflow".into()))?;
    if directory_end > bytes.len() {
        return Err(ImageError::Decode("truncated ICO directory".into()));
    }

    let mut entries = Vec::with_capacity(count);
    for index in 0..count {
        let base = 6 + index * 16;
        let width = if bytes[base] == 0 { 256 } else { bytes[base] as u32 };
        let height = if bytes[base + 1] == 0 { 256 } else { bytes[base + 1] as u32 };
        if bytes[base + 3] != 0 {
            return Err(ImageError::Decode(format!("ICO entry {index} reserved byte must be zero")));
        }
        let bit_depth = read_u16(bytes, base + 6, "entry bit depth")?;
        let size = usize::try_from(read_u32(bytes, base + 8, "entry byte size")?)
            .map_err(|_| ImageError::Decode("ICO entry size does not fit this platform".into()))?;
        let offset = usize::try_from(read_u32(bytes, base + 12, "entry image offset")?)
            .map_err(|_| ImageError::Decode("ICO entry offset does not fit this platform".into()))?;
        if size == 0 {
            return Err(ImageError::Decode(format!("ICO entry {index} has zero byte size")));
        }
        let end = offset
            .checked_add(size)
            .ok_or_else(|| ImageError::Decode("ICO entry payload range overflow".into()))?;
        if offset < directory_end || end > bytes.len() {
            return Err(ImageError::Decode(format!("ICO entry {index} payload is out of bounds")));
        }
        entries.push(Entry { width, height, bit_depth, size, offset, ordinal: index });
    }

    entries.sort_by(|a, b| {
        let a_area = a.width.saturating_mul(a.height);
        let b_area = b.width.saturating_mul(b.height);
        b_area
            .cmp(&a_area)
            .then_with(|| b.bit_depth.cmp(&a.bit_depth))
            .then_with(|| a.ordinal.cmp(&b.ordinal))
    });

    let selected = entries[0];
    let payload = &bytes[selected.offset..selected.offset + selected.size];
    if !payload.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(ImageError::Decode(
            "selected ICO image is DIB-backed; this layer currently supports PNG-backed icon entries".into(),
        ));
    }

    let image = crate::image::decode(payload)?;
    if image.width != selected.width || image.height != selected.height {
        return Err(ImageError::Decode(format!(
            "ICO directory dimensions {}x{} do not match PNG payload {}x{}",
            selected.width, selected.height, image.width, image.height
        )));
    }
    Ok(image)
}

#[derive(Debug, Default, Clone)]
pub struct ImageCache {
    entries: HashMap<String, Result<Rc<RasterImage>, String>>,
}

impl ImageCache {
    pub fn new() -> Self { Self::default() }

    pub fn fetch(&mut self, url: &Url, loader: &dyn ResourceLoader) -> Result<Rc<RasterImage>, String> {
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
            Some(Err(error)) => Some(error),
            _ => None,
        }
    }

    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }

    pub fn insert(&mut self, url: &Url, image: RasterImage) {
        self.entries.insert(url.without_fragment().to_string(), Ok(Rc::new(image)));
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

    fn png_rgba(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            let mut pixels = Vec::with_capacity((width * height * 4) as usize);
            for _ in 0..width * height { pixels.extend_from_slice(&rgba); }
            writer.write_image_data(&pixels).unwrap();
        }
        out
    }

    fn ico(entries: Vec<(u8, u8, u16, Vec<u8>)>) -> Vec<u8> {
        let count = entries.len();
        let mut out = vec![0u8; 6 + count * 16];
        out[2..4].copy_from_slice(&1u16.to_le_bytes());
        out[4..6].copy_from_slice(&(count as u16).to_le_bytes());
        let mut offset = out.len();
        for (i, (width, height, depth, payload)) in entries.iter().enumerate() {
            let base = 6 + i * 16;
            out[base] = *width;
            out[base + 1] = *height;
            out[base + 4..base + 6].copy_from_slice(&1u16.to_le_bytes());
            out[base + 6..base + 8].copy_from_slice(&depth.to_le_bytes());
            out[base + 8..base + 12].copy_from_slice(&(payload.len() as u32).to_le_bytes());
            out[base + 12..base + 16].copy_from_slice(&(offset as u32).to_le_bytes());
            offset += payload.len();
        }
        for (_, _, _, payload) in entries { out.extend_from_slice(&payload); }
        out
    }

    #[test]
    fn decodes_png_backed_icon() {
        let bytes = ico(vec![(1, 1, 32, png_rgba(1, 1, [11, 22, 33, 44]))]);
        let image = decode(&bytes).unwrap();
        assert_eq!((image.width, image.height), (1, 1));
        assert_eq!(image.pixel(0, 0), [11, 22, 33, 44]);
    }

    #[test]
    fn selects_largest_then_deepest_entry() {
        let small = png_rgba(1, 1, [255, 0, 0, 255]);
        let shallow = png_rgba(2, 2, [0, 255, 0, 255]);
        let deep = png_rgba(2, 2, [0, 0, 255, 255]);
        let bytes = ico(vec![(1, 1, 32, small), (2, 2, 8, shallow), (2, 2, 32, deep)]);
        let image = decode(&bytes).unwrap();
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(image.pixel(0, 0), [0, 0, 255, 255]);
    }

    #[test]
    fn rejects_bad_directory_and_dimension_mismatch() {
        let mut bytes = ico(vec![(1, 1, 32, png_rgba(1, 1, [1, 2, 3, 4]))]);
        bytes[18..22].copy_from_slice(&1u32.to_le_bytes());
        assert!(decode(&bytes).is_err());

        let bytes = ico(vec![(2, 2, 32, png_rgba(1, 1, [1, 2, 3, 4]))]);
        assert!(decode(&bytes).is_err());
    }
}
