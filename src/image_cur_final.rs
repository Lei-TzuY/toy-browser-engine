// ============================================================
// image_cur_final.rs — Windows CUR container facade
// ============================================================

use std::collections::HashMap;
use std::rc::Rc;

use crate::net::{LoadError, ResourceLoader, Url};

pub use crate::image_prev18::{ImageError, ImageFormat, RasterImage};

/// A decoded Windows cursor together with its pixel hotspot.
pub struct CursorImage {
    pub image: RasterImage,
    pub hotspot_x: u16,
    pub hotspot_y: u16,
}

impl CursorImage {
    pub fn hotspot(&self) -> (u16, u16) {
        (self.hotspot_x, self.hotspot_y)
    }
}

/// Decode image bytes into straight RGBA8. CUR containers are accepted as
/// ordinary raster images here; callers that need hotspot metadata should use
/// [`decode_cursor`].
pub fn decode(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    if looks_like_cur(bytes) {
        decode_cursor(bytes).map(|cursor| cursor.image)
    } else {
        crate::image_prev18::decode(bytes)
    }
}

fn looks_like_cur(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes[0..4] == [0, 0, 2, 0]
}

fn read_u16(bytes: &[u8], offset: usize, field: &str) -> Result<u16, ImageError> {
    let end = offset
        .checked_add(2)
        .ok_or_else(|| ImageError::Decode(format!("CUR {field} offset overflow")))?;
    let raw: [u8; 2] = bytes
        .get(offset..end)
        .ok_or_else(|| ImageError::Decode(format!("truncated CUR {field}")))?
        .try_into()
        .expect("slice length checked");
    Ok(u16::from_le_bytes(raw))
}

fn read_u32(bytes: &[u8], offset: usize, field: &str) -> Result<u32, ImageError> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| ImageError::Decode(format!("CUR {field} offset overflow")))?;
    let raw: [u8; 4] = bytes
        .get(offset..end)
        .ok_or_else(|| ImageError::Decode(format!("truncated CUR {field}")))?
        .try_into()
        .expect("slice length checked");
    Ok(u32::from_le_bytes(raw))
}

fn read_u16_raw(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn read_u32_raw(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}

#[derive(Clone, Copy, Debug)]
struct CursorEntry {
    width: u32,
    height: u32,
    color_count: u8,
    hotspot_x: u16,
    hotspot_y: u16,
    size: usize,
    offset: usize,
    inferred_depth: u16,
    ordinal: usize,
}

/// Decode a Windows `.cur` container and preserve its hotspot metadata.
pub fn decode_cursor(bytes: &[u8]) -> Result<CursorImage, ImageError> {
    if bytes.len() < 6 {
        return Err(ImageError::Decode("truncated CUR header".into()));
    }
    if read_u16(bytes, 0, "reserved field")? != 0 {
        return Err(ImageError::Decode("CUR reserved field must be zero".into()));
    }
    if read_u16(bytes, 2, "type")? != 2 {
        return Err(ImageError::Decode("CUR type must be 2 (cursor)".into()));
    }

    let count = read_u16(bytes, 4, "image count")? as usize;
    if count == 0 {
        return Err(ImageError::Decode("CUR must contain at least one image".into()));
    }
    let directory_bytes = count
        .checked_mul(16)
        .ok_or_else(|| ImageError::Decode("CUR directory size overflow".into()))?;
    let directory_end = 6usize
        .checked_add(directory_bytes)
        .ok_or_else(|| ImageError::Decode("CUR directory size overflow".into()))?;
    if directory_end > bytes.len() {
        return Err(ImageError::Decode("truncated CUR directory".into()));
    }

    let mut entries = Vec::with_capacity(count);
    for index in 0..count {
        let base = 6 + index * 16;
        let width = if bytes[base] == 0 { 256 } else { bytes[base] as u32 };
        let height = if bytes[base + 1] == 0 { 256 } else { bytes[base + 1] as u32 };
        let color_count = bytes[base + 2];
        if bytes[base + 3] != 0 {
            return Err(ImageError::Decode(format!(
                "CUR entry {index} reserved byte must be zero"
            )));
        }
        let hotspot_x = read_u16(bytes, base + 4, "hotspot x")?;
        let hotspot_y = read_u16(bytes, base + 6, "hotspot y")?;
        if hotspot_x as u32 >= width || hotspot_y as u32 >= height {
            return Err(ImageError::Decode(format!(
                "CUR entry {index} hotspot ({hotspot_x}, {hotspot_y}) is outside {width}x{height} image"
            )));
        }
        let size = usize::try_from(read_u32(bytes, base + 8, "entry byte size")?)
            .map_err(|_| ImageError::Decode("CUR entry size does not fit this platform".into()))?;
        let offset = usize::try_from(read_u32(bytes, base + 12, "entry image offset")?)
            .map_err(|_| ImageError::Decode("CUR entry offset does not fit this platform".into()))?;
        if size == 0 {
            return Err(ImageError::Decode(format!("CUR entry {index} has zero byte size")));
        }
        let end = offset
            .checked_add(size)
            .ok_or_else(|| ImageError::Decode("CUR entry payload range overflow".into()))?;
        if offset < directory_end || end > bytes.len() {
            return Err(ImageError::Decode(format!(
                "CUR entry {index} payload is out of bounds"
            )));
        }
        let payload = &bytes[offset..end];
        entries.push(CursorEntry {
            width,
            height,
            color_count,
            hotspot_x,
            hotspot_y,
            size,
            offset,
            inferred_depth: infer_payload_depth(payload).unwrap_or(0),
            ordinal: index,
        });
    }

    entries.sort_by(|a, b| {
        let a_area = a.width.saturating_mul(a.height);
        let b_area = b.width.saturating_mul(b.height);
        b_area
            .cmp(&a_area)
            .then_with(|| b.inferred_depth.cmp(&a.inferred_depth))
            .then_with(|| a.ordinal.cmp(&b.ordinal))
    });

    let selected = entries[0];
    let payload = &bytes[selected.offset..selected.offset + selected.size];
    let ico = synthesize_single_entry_ico(selected, payload)?;
    let image = crate::image_prev18::decode(&ico)?;

    Ok(CursorImage {
        image,
        hotspot_x: selected.hotspot_x,
        hotspot_y: selected.hotspot_y,
    })
}

fn infer_payload_depth(payload: &[u8]) -> Option<u16> {
    if payload.starts_with(b"\x89PNG\r\n\x1a\n") {
        return infer_png_depth(payload);
    }
    match read_u32_raw(payload, 0)? {
        12 => read_u16_raw(payload, 10),
        size if size >= 40 => read_u16_raw(payload, 14),
        _ => None,
    }
}

fn infer_png_depth(payload: &[u8]) -> Option<u16> {
    if payload.len() < 26 || payload.get(12..16)? != b"IHDR" {
        return None;
    }
    let sample_depth = payload[24] as u16;
    let channels = match payload[25] {
        0 => 1u16,
        2 => 3u16,
        3 => 1u16,
        4 => 2u16,
        6 => 4u16,
        _ => return None,
    };
    sample_depth.checked_mul(channels)
}

fn payload_planes_depth(payload: &[u8]) -> (u16, u16) {
    if payload.starts_with(b"\x89PNG\r\n\x1a\n") {
        return (1, infer_png_depth(payload).unwrap_or(0));
    }
    match read_u32_raw(payload, 0) {
        Some(12) => (
            read_u16_raw(payload, 8).unwrap_or(1),
            read_u16_raw(payload, 10).unwrap_or(0),
        ),
        Some(size) if size >= 40 => (
            read_u16_raw(payload, 12).unwrap_or(1),
            read_u16_raw(payload, 14).unwrap_or(0),
        ),
        _ => (1, 0),
    }
}

fn synthesize_single_entry_ico(
    entry: CursorEntry,
    payload: &[u8],
) -> Result<Vec<u8>, ImageError> {
    let payload_size = u32::try_from(payload.len())
        .map_err(|_| ImageError::Decode("CUR payload is too large for ICO adaptation".into()))?;
    let (planes, depth) = payload_planes_depth(payload);
    let mut ico = vec![0u8; 22];
    ico[2..4].copy_from_slice(&1u16.to_le_bytes());
    ico[4..6].copy_from_slice(&1u16.to_le_bytes());
    ico[6] = if entry.width == 256 { 0 } else { entry.width as u8 };
    ico[7] = if entry.height == 256 { 0 } else { entry.height as u8 };
    ico[8] = entry.color_count;
    ico[10..12].copy_from_slice(&planes.to_le_bytes());
    ico[12..14].copy_from_slice(&depth.to_le_bytes());
    ico[14..18].copy_from_slice(&payload_size.to_le_bytes());
    ico[18..22].copy_from_slice(&22u32.to_le_bytes());
    ico.extend_from_slice(payload);
    Ok(ico)
}

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

    pub fn insert(&mut self, url: &Url, image: RasterImage) {
        self.entries
            .insert(url.without_fragment().to_string(), Ok(Rc::new(image)));
    }
}

fn load_and_decode(url: &Url, loader: &dyn ResourceLoader) -> Result<Rc<RasterImage>, String> {
    let resource = loader
        .load(url)
        .map_err(|error: LoadError| error.to_string())?;
    decode(&resource.bytes)
        .map(Rc::new)
        .map_err(|error| format!("{url}: {error}"))
}

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
        let outcome = load_and_decode_cursor(url, loader);
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

fn load_and_decode_cursor(
    url: &Url,
    loader: &dyn ResourceLoader,
) -> Result<Rc<CursorImage>, String> {
    let resource = loader
        .load(url)
        .map_err(|error: LoadError| error.to_string())?;
    decode_cursor(&resource.bytes)
        .map(Rc::new)
        .map_err(|error| format!("{url}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dib24(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
        let xor_stride = ((width as usize * 24 + 31) / 32) * 4;
        let and_stride = ((width as usize + 31) / 32) * 4;
        let mut out = vec![0u8; 40 + xor_stride * height as usize + and_stride * height as usize];
        out[0..4].copy_from_slice(&40u32.to_le_bytes());
        out[4..8].copy_from_slice(&(width as i32).to_le_bytes());
        out[8..12].copy_from_slice(&((height as i32) * 2).to_le_bytes());
        out[12..14].copy_from_slice(&1u16.to_le_bytes());
        out[14..16].copy_from_slice(&24u16.to_le_bytes());
        for file_y in 0..height as usize {
            let row = 40 + file_y * xor_stride;
            for x in 0..width as usize {
                let base = row + x * 3;
                out[base..base + 3].copy_from_slice(&[rgb[2], rgb[1], rgb[0]]);
            }
        }
        out
    }

    fn cur(entries: Vec<(u8, u8, u16, u16, Vec<u8>)>) -> Vec<u8> {
        let count = entries.len();
        let mut out = vec![0u8; 6 + count * 16];
        out[2..4].copy_from_slice(&2u16.to_le_bytes());
        out[4..6].copy_from_slice(&(count as u16).to_le_bytes());
        let mut offset = out.len();
        for (index, (width, height, hot_x, hot_y, payload)) in entries.iter().enumerate() {
            let base = 6 + index * 16;
            out[base] = *width;
            out[base + 1] = *height;
            out[base + 4..base + 6].copy_from_slice(&hot_x.to_le_bytes());
            out[base + 6..base + 8].copy_from_slice(&hot_y.to_le_bytes());
            out[base + 8..base + 12].copy_from_slice(&(payload.len() as u32).to_le_bytes());
            out[base + 12..base + 16].copy_from_slice(&(offset as u32).to_le_bytes());
            offset += payload.len();
        }
        for (_, _, _, _, payload) in entries {
            out.extend_from_slice(&payload);
        }
        out
    }

    #[test]
    fn decodes_dib_cursor_and_preserves_hotspot() {
        let bytes = cur(vec![(2, 2, 1, 1, dib24(2, 2, [10, 20, 30]))]);
        let cursor = decode_cursor(&bytes).expect("DIB CUR");
        assert_eq!(cursor.hotspot(), (1, 1));
        assert_eq!((cursor.image.width, cursor.image.height), (2, 2));
        assert_eq!(cursor.image.pixel(0, 0), [10, 20, 30, 255]);
        assert_eq!(decode(&bytes).unwrap().pixel(1, 1), [10, 20, 30, 255]);
    }

    #[test]
    fn selects_largest_cursor_and_associated_hotspot() {
        let bytes = cur(vec![
            (1, 1, 0, 0, dib24(1, 1, [255, 0, 0])),
            (2, 2, 1, 0, dib24(2, 2, [0, 255, 0])),
        ]);
        let cursor = decode_cursor(&bytes).unwrap();
        assert_eq!(cursor.hotspot(), (1, 0));
        assert_eq!((cursor.image.width, cursor.image.height), (2, 2));
        assert_eq!(cursor.image.pixel(0, 0), [0, 255, 0, 255]);
    }

    #[test]
    fn rejects_invalid_hotspot_and_payload_ranges() {
        let bad_hotspot = cur(vec![(1, 1, 1, 0, dib24(1, 1, [0, 0, 0]))]);
        assert!(decode_cursor(&bad_hotspot).is_err());

        let mut truncated = cur(vec![(1, 1, 0, 0, dib24(1, 1, [0, 0, 0]))]);
        truncated.truncate(truncated.len() - 1);
        assert!(decode_cursor(&truncated).is_err());
    }
}
