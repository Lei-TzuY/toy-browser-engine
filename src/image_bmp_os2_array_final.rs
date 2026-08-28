// ============================================================
// image_bmp_os2_array_final.rs — OS/2 Bitmap Array facade
// ============================================================

use std::collections::HashMap;
use std::rc::Rc;

use crate::net::{LoadError, ResourceLoader, Url};

pub use crate::image_prev13::{ImageError, ImageFormat, RasterImage};

/// Decode image bytes into straight RGBA8, adding OS/2 Bitmap Array (`BA`) support.
///
/// The public image API has no display-device metrics, so the decoder follows the
/// OS/2 fallback rule and decodes the first bitmap in the array. Each array entry
/// stores a complete `BM` bitmap whose pixel offset is relative to the containing
/// file; it is normalized to an ordinary standalone bitmap before delegating to
/// the existing BMP stack.
pub fn decode(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    if bytes.starts_with(b"BA") {
        decode_bitmap_array(bytes)
    } else {
        crate::image_prev13::decode(bytes)
    }
}

fn read_u32(bytes: &[u8], offset: usize, field: &str) -> Result<u32, ImageError> {
    let raw = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| ImageError::Decode(format!("truncated OS/2 bitmap array {field}")))?;
    Ok(u32::from_le_bytes(raw.try_into().unwrap()))
}

fn decode_bitmap_array(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    const ARRAY_HEADER: usize = 14;
    const FILE_HEADER: usize = 14;

    if bytes.len() < ARRAY_HEADER + FILE_HEADER {
        return Err(ImageError::Decode("truncated OS/2 bitmap array header".into()));
    }
    if &bytes[0..2] != b"BA" {
        return Err(ImageError::Decode("invalid OS/2 bitmap array signature".into()));
    }

    let next = usize::try_from(read_u32(bytes, 6, "next-entry offset")?)
        .map_err(|_| ImageError::Decode("OS/2 bitmap array next-entry offset does not fit this platform".into()))?;
    let bitmap_start = ARRAY_HEADER;
    if bytes.get(bitmap_start..bitmap_start + 2) != Some(b"BM".as_slice()) {
        return Err(ImageError::Decode("OS/2 bitmap array entry is missing its BM header".into()));
    }

    let entry_end = if next == 0 {
        bytes.len()
    } else {
        if next <= bitmap_start + FILE_HEADER || next > bytes.len() {
            return Err(ImageError::Decode("invalid OS/2 bitmap array next-entry offset".into()));
        }
        if bytes.get(next..next + 2) != Some(b"BA".as_slice()) {
            return Err(ImageError::Decode("OS/2 bitmap array next entry is missing its BA header".into()));
        }
        next
    };

    let pixel_absolute = usize::try_from(read_u32(bytes, bitmap_start + 10, "bitmap-data offset")?)
        .map_err(|_| ImageError::Decode("OS/2 bitmap array bitmap-data offset does not fit this platform".into()))?;
    if pixel_absolute < bitmap_start + FILE_HEADER || pixel_absolute > entry_end {
        return Err(ImageError::Decode("invalid OS/2 bitmap array bitmap-data offset".into()));
    }

    let local_offset = pixel_absolute - bitmap_start;
    let local_offset_u32 = u32::try_from(local_offset)
        .map_err(|_| ImageError::Decode("OS/2 bitmap array local bitmap offset overflow".into()))?;
    let local_size = entry_end - bitmap_start;
    let local_size_u32 = u32::try_from(local_size)
        .map_err(|_| ImageError::Decode("OS/2 bitmap array entry size overflow".into()))?;

    let mut bitmap = bytes[bitmap_start..entry_end].to_vec();
    bitmap[2..6].copy_from_slice(&local_size_u32.to_le_bytes());
    bitmap[10..14].copy_from_slice(&local_offset_u32.to_le_bytes());
    crate::image_prev13::decode(&bitmap)
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

    fn core24(rgb: [u8; 3]) -> Vec<u8> {
        let mut bm = vec![0u8; 26];
        bm[0..2].copy_from_slice(b"BM");
        bm[2..6].copy_from_slice(&30u32.to_le_bytes());
        bm[10..14].copy_from_slice(&26u32.to_le_bytes());
        bm[14..18].copy_from_slice(&12u32.to_le_bytes());
        bm[18..20].copy_from_slice(&1u16.to_le_bytes());
        bm[20..22].copy_from_slice(&1u16.to_le_bytes());
        bm[22..24].copy_from_slice(&1u16.to_le_bytes());
        bm[24..26].copy_from_slice(&24u16.to_le_bytes());
        bm.extend_from_slice(&[rgb[2], rgb[1], rgb[0], 0]);
        bm
    }

    fn array_entry(mut bm: Vec<u8>, next: u32, absolute_start: usize) -> Vec<u8> {
        let local_off = u32::from_le_bytes(bm[10..14].try_into().unwrap()) as usize;
        bm[10..14].copy_from_slice(&((absolute_start + 14 + local_off) as u32).to_le_bytes());
        let mut out = vec![0u8; 14];
        out[0..2].copy_from_slice(b"BA");
        out[6..10].copy_from_slice(&next.to_le_bytes());
        out.extend_from_slice(&bm);
        out
    }

    #[test]
    fn decodes_first_bitmap_with_absolute_pixel_offset() {
        let bytes = array_entry(core24([12, 34, 56]), 0, 0);
        let image = decode(&bytes).unwrap();
        assert_eq!((image.width, image.height), (1, 1));
        assert_eq!(image.pixel(0, 0), [12, 34, 56, 255]);
    }

    #[test]
    fn first_bitmap_is_fallback_when_array_has_multiple_entries() {
        let first_len = 14 + core24([255, 0, 0]).len();
        let mut bytes = array_entry(core24([255, 0, 0]), first_len as u32, 0);
        bytes.extend_from_slice(&array_entry(core24([0, 255, 0]), 0, first_len));
        let image = decode(&bytes).unwrap();
        assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn rejects_invalid_array_links_and_offsets() {
        let mut bad_next = array_entry(core24([1, 2, 3]), 20, 0);
        assert!(decode(&bad_next).is_err());

        bad_next = array_entry(core24([1, 2, 3]), 44, 0);
        assert!(decode(&bad_next).is_err());

        let mut bad_pixels = array_entry(core24([1, 2, 3]), 0, 0);
        bad_pixels[24..28].copy_from_slice(&4u32.to_le_bytes());
        assert!(decode(&bad_pixels).is_err());
    }
}
