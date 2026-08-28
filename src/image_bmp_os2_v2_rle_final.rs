// ============================================================
// image_bmp_os2_v2_rle_final.rs — OS/2 2.x RLE BMP facade
// ============================================================

use std::collections::HashMap;
use std::rc::Rc;

use crate::net::{LoadError, ResourceLoader, Url};

pub use crate::image_prev12::{ImageError, ImageFormat, RasterImage};

/// Decode image bytes into straight RGBA8, adding OS/2 2.x RLE4/RLE8.
pub fn decode(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    if is_os2_v2_rle(bytes) {
        decode_os2_v2_rle(bytes)
    } else {
        crate::image_prev12::decode(bytes)
    }
}

fn read_u16(bytes: &[u8], offset: usize, field: &str) -> Result<u16, ImageError> {
    let raw = bytes.get(offset..offset + 2)
        .ok_or_else(|| ImageError::Decode(format!("truncated OS/2 2.x RLE BMP {field}")))?;
    Ok(u16::from_le_bytes(raw.try_into().unwrap()))
}

fn read_u32(bytes: &[u8], offset: usize, field: &str) -> Result<u32, ImageError> {
    let raw = bytes.get(offset..offset + 4)
        .ok_or_else(|| ImageError::Decode(format!("truncated OS/2 2.x RLE BMP {field}")))?;
    Ok(u32::from_le_bytes(raw.try_into().unwrap()))
}

fn is_os2_v2_rle(bytes: &[u8]) -> bool {
    if !bytes.starts_with(b"BM") || bytes.len() < 34 {
        return false;
    }
    let dib = u32::from_le_bytes(bytes[14..18].try_into().unwrap());
    let compression = u32::from_le_bytes(bytes[30..34].try_into().unwrap());
    dib == 64 && matches!(compression, 1 | 2)
}

fn decode_os2_v2_rle(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    const HEADER_END: usize = 78;
    if bytes.len() < HEADER_END {
        return Err(ImageError::Decode("truncated OS/2 2.x RLE BMP header".into()));
    }

    let pixel_offset = read_u32(bytes, 10, "pixel offset")? as usize;
    if pixel_offset < HEADER_END || pixel_offset > bytes.len() {
        return Err(ImageError::Decode("invalid OS/2 2.x RLE BMP pixel offset".into()));
    }

    let width_u32 = read_u32(bytes, 18, "width")?;
    let height_u32 = read_u32(bytes, 22, "height")?;
    if width_u32 == 0 || height_u32 == 0 {
        return Err(ImageError::Decode("OS/2 2.x RLE BMP dimensions must be non-zero".into()));
    }
    let width = usize::try_from(width_u32)
        .map_err(|_| ImageError::Decode("OS/2 2.x RLE BMP width does not fit this platform".into()))?;
    let height = usize::try_from(height_u32)
        .map_err(|_| ImageError::Decode("OS/2 2.x RLE BMP height does not fit this platform".into()))?;

    if read_u16(bytes, 26, "planes")? != 1 {
        return Err(ImageError::Decode("OS/2 2.x RLE BMP requires one plane".into()));
    }
    let depth = read_u16(bytes, 28, "bits per pixel")?;
    let compression = read_u32(bytes, 30, "compression")?;
    match (compression, depth) {
        (1, 8) | (2, 4) => {}
        (1, _) => return Err(ImageError::Decode("OS/2 2.x RLE8 requires 8-bit pixels".into())),
        (2, _) => return Err(ImageError::Decode("OS/2 2.x RLE4 requires 4-bit pixels".into())),
        _ => return Err(ImageError::Decode("unsupported OS/2 2.x RLE compression".into())),
    }

    // Compressed OS/2 bitmaps are defined in the conventional bottom-up order.
    // Reject direction=1 rather than ambiguously reinterpreting cursor movement.
    if read_u16(bytes, 58, "recording direction")? != 0 {
        return Err(ImageError::Decode("compressed OS/2 2.x BMP must be bottom-up".into()));
    }

    let max_entries = if depth == 8 { 256usize } else { 16usize };
    let colors_used = read_u32(bytes, 46, "colors used")? as usize;
    let palette_entries = if colors_used == 0 { max_entries } else { colors_used };
    if palette_entries == 0 || palette_entries > max_entries {
        return Err(ImageError::Decode("OS/2 2.x RLE BMP palette exceeds bit-depth capacity".into()));
    }
    let palette_bytes = palette_entries.checked_mul(4)
        .ok_or_else(|| ImageError::Decode("OS/2 2.x RLE BMP palette size overflow".into()))?;
    let palette_end = HEADER_END.checked_add(palette_bytes)
        .ok_or_else(|| ImageError::Decode("OS/2 2.x RLE BMP palette offset overflow".into()))?;
    if palette_end > pixel_offset || palette_end > bytes.len() {
        return Err(ImageError::Decode("truncated OS/2 2.x RLE BMP RGB2 palette".into()));
    }
    let palette: Vec<[u8; 4]> = (0..palette_entries).map(|i| {
        let base = HEADER_END + i * 4;
        [bytes[base + 2], bytes[base + 1], bytes[base], 255]
    }).collect();

    let pixel_count = width.checked_mul(height)
        .ok_or_else(|| ImageError::Decode("OS/2 2.x RLE BMP pixel count overflow".into()))?;
    let mut indices = vec![0u8; pixel_count];
    let mut pos = pixel_offset;
    let mut x = 0usize;
    let mut y = 0usize; // file-space row: 0 is bottom
    let mut ended = false;

    while pos < bytes.len() {
        let count = *bytes.get(pos).ok_or_else(|| ImageError::Decode("truncated OS/2 2.x RLE command".into()))?;
        let value = *bytes.get(pos + 1).ok_or_else(|| ImageError::Decode("truncated OS/2 2.x RLE command".into()))?;
        pos += 2;

        if count != 0 {
            let run = count as usize;
            ensure_run_bounds(width, height, x, y, run)?;
            if compression == 1 {
                ensure_palette_index(value as usize, palette.len())?;
                for _ in 0..run {
                    indices[y * width + x] = value;
                    x += 1;
                }
            } else {
                let hi = value >> 4;
                let lo = value & 0x0f;
                ensure_palette_index(hi as usize, palette.len())?;
                ensure_palette_index(lo as usize, palette.len())?;
                for i in 0..run {
                    indices[y * width + x] = if i % 2 == 0 { hi } else { lo };
                    x += 1;
                }
            }
            continue;
        }

        match value {
            0 => { // end of line
                if y >= height { return Err(ImageError::Decode("OS/2 2.x RLE EOL beyond image".into())); }
                x = 0;
                y += 1;
            }
            1 => { ended = true; break; }
            2 => { // delta
                let dx = *bytes.get(pos).ok_or_else(|| ImageError::Decode("truncated OS/2 2.x RLE delta".into()))? as usize;
                let dy = *bytes.get(pos + 1).ok_or_else(|| ImageError::Decode("truncated OS/2 2.x RLE delta".into()))? as usize;
                pos += 2;
                x = x.checked_add(dx).ok_or_else(|| ImageError::Decode("OS/2 2.x RLE delta overflow".into()))?;
                y = y.checked_add(dy).ok_or_else(|| ImageError::Decode("OS/2 2.x RLE delta overflow".into()))?;
                if x >= width || y >= height {
                    return Err(ImageError::Decode("OS/2 2.x RLE delta moves outside image".into()));
                }
            }
            literal => {
                let n = literal as usize;
                ensure_run_bounds(width, height, x, y, n)?;
                if compression == 1 {
                    let data = bytes.get(pos..pos + n)
                        .ok_or_else(|| ImageError::Decode("truncated OS/2 2.x RLE8 absolute run".into()))?;
                    for &idx in data {
                        ensure_palette_index(idx as usize, palette.len())?;
                        indices[y * width + x] = idx;
                        x += 1;
                    }
                    pos += n;
                    if n & 1 != 0 {
                        if pos >= bytes.len() { return Err(ImageError::Decode("truncated OS/2 2.x RLE8 absolute padding".into())); }
                        pos += 1;
                    }
                } else {
                    let packed = (n + 1) / 2;
                    let data = bytes.get(pos..pos + packed)
                        .ok_or_else(|| ImageError::Decode("truncated OS/2 2.x RLE4 absolute run".into()))?;
                    for i in 0..n {
                        let byte = data[i / 2];
                        let idx = if i % 2 == 0 { byte >> 4 } else { byte & 0x0f };
                        ensure_palette_index(idx as usize, palette.len())?;
                        indices[y * width + x] = idx;
                        x += 1;
                    }
                    pos += packed;
                    if packed & 1 != 0 {
                        if pos >= bytes.len() { return Err(ImageError::Decode("truncated OS/2 2.x RLE4 absolute padding".into())); }
                        pos += 1;
                    }
                }
            }
        }
    }

    if !ended {
        return Err(ImageError::Decode("OS/2 2.x RLE BMP missing end-of-bitmap marker".into()));
    }

    let rgba_len = pixel_count.checked_mul(4)
        .ok_or_else(|| ImageError::Decode("OS/2 2.x RLE BMP RGBA size overflow".into()))?;
    let mut pixels = vec![0u8; rgba_len];
    for file_y in 0..height {
        let image_y = height - 1 - file_y;
        for px in 0..width {
            let idx = indices[file_y * width + px] as usize;
            let rgba = *palette.get(idx)
                .ok_or_else(|| ImageError::Decode("OS/2 2.x RLE BMP palette index out of range".into()))?;
            let dst = (image_y * width + px) * 4;
            pixels[dst..dst + 4].copy_from_slice(&rgba);
        }
    }

    Ok(RasterImage::new(width_u32, height_u32, pixels))
}

fn ensure_run_bounds(width: usize, height: usize, x: usize, y: usize, len: usize) -> Result<(), ImageError> {
    if y >= height || x > width || len > width.saturating_sub(x) {
        Err(ImageError::Decode("OS/2 2.x RLE run exceeds scanline".into()))
    } else {
        Ok(())
    }
}

fn ensure_palette_index(index: usize, len: usize) -> Result<(), ImageError> {
    if index >= len {
        Err(ImageError::Decode("OS/2 2.x RLE BMP palette index out of range".into()))
    } else {
        Ok(())
    }
}

#[derive(Debug, Default, Clone)]
pub struct ImageCache {
    entries: HashMap<String, Result<Rc<RasterImage>, String>>,
}

impl ImageCache {
    pub fn new() -> Self { Self::default() }
    pub fn fetch(&mut self, url: &Url, loader: &dyn ResourceLoader) -> Result<Rc<RasterImage>, String> {
        let key = url.without_fragment().to_string();
        if let Some(entry) = self.entries.get(&key) { return entry.clone(); }
        let outcome = load_and_decode(url, loader);
        self.entries.insert(key, outcome.clone());
        outcome
    }
    pub fn get(&self, url: &Url) -> Option<Rc<RasterImage>> {
        self.entries.get(&url.without_fragment().to_string()).and_then(|e| e.as_ref().ok().cloned())
    }
    pub fn error(&self, url: &Url) -> Option<&str> {
        match self.entries.get(&url.without_fragment().to_string()) { Some(Err(e)) => Some(e), _ => None }
    }
    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
    pub fn insert(&mut self, url: &Url, image: RasterImage) {
        self.entries.insert(url.without_fragment().to_string(), Ok(Rc::new(image)));
    }
}

fn load_and_decode(url: &Url, loader: &dyn ResourceLoader) -> Result<Rc<RasterImage>, String> {
    let resource = loader.load(url).map_err(|error: LoadError| error.to_string())?;
    decode(&resource.bytes).map(Rc::new).map_err(|error| format!("{url}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bmp(width: u32, height: u32, depth: u16, compression: u32, palette: &[[u8; 3]], stream: &[u8]) -> Vec<u8> {
        let offset = 78 + palette.len() * 4;
        let mut out = vec![0u8; offset];
        out[0..2].copy_from_slice(b"BM");
        out[2..6].copy_from_slice(&((offset + stream.len()) as u32).to_le_bytes());
        out[10..14].copy_from_slice(&(offset as u32).to_le_bytes());
        out[14..18].copy_from_slice(&64u32.to_le_bytes());
        out[18..22].copy_from_slice(&width.to_le_bytes());
        out[22..26].copy_from_slice(&height.to_le_bytes());
        out[26..28].copy_from_slice(&1u16.to_le_bytes());
        out[28..30].copy_from_slice(&depth.to_le_bytes());
        out[30..34].copy_from_slice(&compression.to_le_bytes());
        out[46..50].copy_from_slice(&(palette.len() as u32).to_le_bytes());
        for (i, rgb) in palette.iter().enumerate() {
            let base = 78 + i * 4;
            out[base..base + 4].copy_from_slice(&[rgb[2], rgb[1], rgb[0], 0]);
        }
        out.extend_from_slice(stream);
        out
    }

    #[test]
    fn decodes_rle8_encoded_absolute_and_bottom_up_rows() {
        let p = [[0,0,0],[255,0,0],[0,255,0],[0,0,255]];
        let bytes = bmp(4, 2, 8, 1, &p, &[
            4, 3, 0, 0,                  // bottom: blue x4, EOL
            0, 4, 1, 2, 1, 2, 0, 0,     // top: absolute red/green/red/green, EOL
            0, 1,
        ]);
        let image = decode(&bytes).unwrap();
        assert_eq!(image.pixel(0, 0), [255,0,0,255]);
        assert_eq!(image.pixel(1, 0), [0,255,0,255]);
        assert_eq!(image.pixel(0, 1), [0,0,255,255]);
    }

    #[test]
    fn decodes_rle4_encoded_absolute_and_delta() {
        let p = [[0,0,0],[255,0,0],[0,255,0],[0,0,255]];
        let bytes = bmp(5, 2, 4, 2, &p, &[
            5, 0x12, 0, 0,               // bottom alternating red/green
            0, 2, 1, 0,                  // top: skip one pixel
            0, 4, 0x31, 0x23, 0, 0,      // four absolute nibbles + EOL
            0, 1,
        ]);
        let image = decode(&bytes).unwrap();
        assert_eq!(image.pixel(0, 0), [0,0,0,255]);
        assert_eq!(image.pixel(1, 0), [0,0,255,255]);
        assert_eq!(image.pixel(4, 0), [0,0,255,255]);
        assert_eq!(image.pixel(0, 1), [255,0,0,255]);
        assert_eq!(image.pixel(1, 1), [0,255,0,255]);
    }

    #[test]
    fn rejects_missing_eob_out_of_bounds_and_top_down() {
        let p = [[0,0,0],[255,0,0]];
        assert!(decode(&bmp(1,1,8,1,&p,&[1,1])).is_err());
        assert!(decode(&bmp(1,1,8,1,&p,&[2,1,0,1])).is_err());
        let mut top_down = bmp(1,1,8,1,&p,&[1,1,0,1]);
        top_down[58..60].copy_from_slice(&1u16.to_le_bytes());
        assert!(decode(&top_down).is_err());
    }
}
