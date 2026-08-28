// ============================================================
// image_bmp_os2_v2_final.rs — OS/2 2.x BITMAPINFOHEADER2 BMP facade
// ============================================================

use std::collections::HashMap;
use std::rc::Rc;

use crate::net::{LoadError, ResourceLoader, Url};

pub use crate::image_prev11::{ImageError, ImageFormat, RasterImage};

/// Decode image bytes into straight RGBA8, adding OS/2 2.x
/// BITMAPINFOHEADER2 support on top of the existing image stack.
pub fn decode(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    if is_os2_v2_bmp(bytes) {
        decode_os2_v2_bmp(bytes)
    } else {
        crate::image_prev11::decode(bytes)
    }
}

fn is_os2_v2_bmp(bytes: &[u8]) -> bool {
    bytes.starts_with(b"BM") && read_u32_raw(bytes, 14) == Some(64)
}

fn read_u16_raw(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(offset..offset + 2)?.try_into().ok()?))
}

fn read_u32_raw(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(offset..offset + 4)?.try_into().ok()?))
}

fn u16_le(bytes: &[u8], offset: usize, field: &str) -> Result<u16, ImageError> {
    read_u16_raw(bytes, offset)
        .ok_or_else(|| ImageError::Decode(format!("truncated OS/2 2.x BMP {field}")))
}

fn u32_le(bytes: &[u8], offset: usize, field: &str) -> Result<u32, ImageError> {
    read_u32_raw(bytes, offset)
        .ok_or_else(|| ImageError::Decode(format!("truncated OS/2 2.x BMP {field}")))
}

fn decode_os2_v2_bmp(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    const FILE_HEADER: usize = 14;
    const INFO2_HEADER: usize = 64;
    const HEADER_END: usize = FILE_HEADER + INFO2_HEADER;

    if bytes.len() < HEADER_END {
        return Err(ImageError::Decode("truncated OS/2 2.x BMP header".into()));
    }

    let pixel_offset = u32_le(bytes, 10, "pixel offset")? as usize;
    if pixel_offset < HEADER_END || pixel_offset > bytes.len() {
        return Err(ImageError::Decode("invalid OS/2 2.x BMP pixel offset".into()));
    }

    let width_u32 = u32_le(bytes, 18, "width")?;
    let height_u32 = u32_le(bytes, 22, "height")?;
    if width_u32 == 0 || height_u32 == 0 {
        return Err(ImageError::Decode("OS/2 2.x BMP dimensions must be non-zero".into()));
    }
    let width = usize::try_from(width_u32)
        .map_err(|_| ImageError::Decode("OS/2 2.x BMP width does not fit this platform".into()))?;
    let height = usize::try_from(height_u32)
        .map_err(|_| ImageError::Decode("OS/2 2.x BMP height does not fit this platform".into()))?;

    if u16_le(bytes, 26, "planes")? != 1 {
        return Err(ImageError::Decode("OS/2 2.x BMP requires one plane".into()));
    }
    let depth = u16_le(bytes, 28, "bits per pixel")?;
    if !matches!(depth, 1 | 4 | 8 | 24) {
        return Err(ImageError::Decode(format!(
            "unsupported OS/2 2.x BMP bit depth {depth}"
        )));
    }
    if u32_le(bytes, 30, "compression")? != 0 {
        return Err(ImageError::Decode(
            "unsupported compressed OS/2 2.x BMP".into(),
        ));
    }

    // OS/2 2.x records scanline direction explicitly instead of using a
    // signed height. 0 is bottom-up; 1 is top-down.
    let recording = u16_le(bytes, 58, "recording direction")?;
    let top_down = match recording {
        0 => false,
        1 => true,
        other => {
            return Err(ImageError::Decode(format!(
                "unsupported OS/2 2.x BMP recording direction {other}"
            )))
        }
    };

    let max_palette_entries = match depth {
        1 => 2usize,
        4 => 16usize,
        8 => 256usize,
        24 => 0usize,
        _ => unreachable!(),
    };
    let colors_used = u32_le(bytes, 46, "colors used")? as usize;
    let palette_entries = if max_palette_entries == 0 {
        0
    } else if colors_used == 0 {
        max_palette_entries
    } else {
        if colors_used > max_palette_entries {
            return Err(ImageError::Decode(
                "OS/2 2.x BMP palette exceeds bit-depth capacity".into(),
            ));
        }
        colors_used
    };

    let palette_len = palette_entries
        .checked_mul(4)
        .ok_or_else(|| ImageError::Decode("OS/2 2.x BMP palette size overflow".into()))?;
    let palette_end = HEADER_END
        .checked_add(palette_len)
        .ok_or_else(|| ImageError::Decode("OS/2 2.x BMP palette offset overflow".into()))?;
    if palette_end > pixel_offset || palette_end > bytes.len() {
        return Err(ImageError::Decode("truncated OS/2 2.x BMP RGB2 palette".into()));
    }

    let palette: Vec<[u8; 4]> = (0..palette_entries)
        .map(|index| {
            let base = HEADER_END + index * 4;
            [bytes[base + 2], bytes[base + 1], bytes[base], 255]
        })
        .collect();

    let row_bits = width
        .checked_mul(depth as usize)
        .ok_or_else(|| ImageError::Decode("OS/2 2.x BMP row bit count overflow".into()))?;
    let row_payload = row_bits
        .checked_add(7)
        .map(|bits| bits / 8)
        .ok_or_else(|| ImageError::Decode("OS/2 2.x BMP row payload overflow".into()))?;
    let row_stride = row_payload
        .checked_add(3)
        .map(|bytes| bytes & !3)
        .ok_or_else(|| ImageError::Decode("OS/2 2.x BMP row padding overflow".into()))?;
    let raster_len = row_stride
        .checked_mul(height)
        .ok_or_else(|| ImageError::Decode("OS/2 2.x BMP raster size overflow".into()))?;
    let raster_end = pixel_offset
        .checked_add(raster_len)
        .ok_or_else(|| ImageError::Decode("OS/2 2.x BMP raster offset overflow".into()))?;
    if raster_end > bytes.len() {
        return Err(ImageError::Decode("truncated OS/2 2.x BMP raster".into()));
    }

    let rgba_len = width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| ImageError::Decode("OS/2 2.x BMP RGBA size overflow".into()))?;
    let mut pixels = vec![0u8; rgba_len];

    for file_y in 0..height {
        let image_y = if top_down { file_y } else { height - 1 - file_y };
        let row = pixel_offset + file_y * row_stride;
        for x in 0..width {
            let rgba = match depth {
                1 => {
                    let byte = bytes[row + x / 8];
                    let index = ((byte >> (7 - (x % 8))) & 1) as usize;
                    *palette.get(index).ok_or_else(|| {
                        ImageError::Decode("OS/2 2.x BMP palette index out of range".into())
                    })?
                }
                4 => {
                    let byte = bytes[row + x / 2];
                    let index = if x % 2 == 0 { byte >> 4 } else { byte & 0x0f } as usize;
                    *palette.get(index).ok_or_else(|| {
                        ImageError::Decode("OS/2 2.x BMP palette index out of range".into())
                    })?
                }
                8 => {
                    let index = bytes[row + x] as usize;
                    *palette.get(index).ok_or_else(|| {
                        ImageError::Decode("OS/2 2.x BMP palette index out of range".into())
                    })?
                }
                24 => {
                    let src = row + x * 3;
                    [bytes[src + 2], bytes[src + 1], bytes[src], 255]
                }
                _ => unreachable!(),
            };
            let dst = (image_y * width + x) * 4;
            pixels[dst..dst + 4].copy_from_slice(&rgba);
        }
    }

    Ok(RasterImage::new(width_u32, height_u32, pixels))
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

    fn info2_bmp(
        width: u32,
        height: u32,
        depth: u16,
        recording: u16,
        palette: &[[u8; 3]],
        raster: &[u8],
    ) -> Vec<u8> {
        let offset = 78 + palette.len() * 4;
        let mut out = vec![0u8; offset];
        out[0..2].copy_from_slice(b"BM");
        out[2..6].copy_from_slice(&((offset + raster.len()) as u32).to_le_bytes());
        out[10..14].copy_from_slice(&(offset as u32).to_le_bytes());
        out[14..18].copy_from_slice(&64u32.to_le_bytes());
        out[18..22].copy_from_slice(&width.to_le_bytes());
        out[22..26].copy_from_slice(&height.to_le_bytes());
        out[26..28].copy_from_slice(&1u16.to_le_bytes());
        out[28..30].copy_from_slice(&depth.to_le_bytes());
        out[46..50].copy_from_slice(&(palette.len() as u32).to_le_bytes());
        out[58..60].copy_from_slice(&recording.to_le_bytes());
        for (i, rgb) in palette.iter().enumerate() {
            let base = 78 + i * 4;
            out[base..base + 4].copy_from_slice(&[rgb[2], rgb[1], rgb[0], 0]);
        }
        out.extend_from_slice(raster);
        out
    }

    #[test]
    fn decodes_rgb2_palette_and_packed_pixels() {
        let palette = [[0, 0, 0], [255, 0, 0], [0, 255, 0]];
        let bytes = info2_bmp(2, 1, 4, 0, &palette, &[0x12, 0, 0, 0]);
        let image = decode(&bytes).expect("OS/2 2.x indexed BMP");
        assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(image.pixel(1, 0), [0, 255, 0, 255]);
    }

    #[test]
    fn honors_recording_direction_for_24_bit_rows() {
        let top_down = info2_bmp(
            1,
            2,
            24,
            1,
            &[],
            &[
                0, 0, 255, 0, // top red
                255, 0, 0, 0, // bottom blue
            ],
        );
        let image = decode(&top_down).expect("top-down OS/2 2.x BMP");
        assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(image.pixel(0, 1), [0, 0, 255, 255]);
    }

    #[test]
    fn rejects_bad_palette_compression_and_truncation() {
        let palette = [[0, 0, 0], [255, 0, 0]];
        let mut bad_index = info2_bmp(1, 1, 8, 0, &palette, &[3, 0, 0, 0]);
        assert!(decode(&bad_index).is_err());

        bad_index[30..34].copy_from_slice(&1u32.to_le_bytes());
        assert!(decode(&bad_index).is_err());

        let mut truncated = info2_bmp(1, 1, 1, 0, &palette, &[0; 4]);
        truncated.pop();
        assert!(decode(&truncated).is_err());
    }
}
