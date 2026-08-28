// ============================================================
// image_bmp_os2_final.rs — OS/2 BITMAPCOREHEADER BMP facade
// ============================================================

use std::collections::HashMap;
use std::rc::Rc;

use crate::net::{LoadError, ResourceLoader, Url};

pub use crate::image_prev10::{ImageError, ImageFormat, RasterImage};

/// Decode image bytes into straight RGBA8, adding OS/2 1.x
/// BITMAPCOREHEADER support on top of the existing image stack.
pub fn decode(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    if is_os2_core_bmp(bytes) {
        decode_os2_core_bmp(bytes)
    } else {
        crate::image_prev10::decode(bytes)
    }
}

fn is_os2_core_bmp(bytes: &[u8]) -> bool {
    bytes.starts_with(b"BM") && read_u32_raw(bytes, 14) == Some(12)
}

fn read_u16_raw(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(offset..offset + 2)?.try_into().ok()?))
}

fn read_u32_raw(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(offset..offset + 4)?.try_into().ok()?))
}

fn u16_le(bytes: &[u8], offset: usize, field: &str) -> Result<u16, ImageError> {
    read_u16_raw(bytes, offset)
        .ok_or_else(|| ImageError::Decode(format!("truncated OS/2 BMP {field}")))
}

fn u32_le(bytes: &[u8], offset: usize, field: &str) -> Result<u32, ImageError> {
    read_u32_raw(bytes, offset)
        .ok_or_else(|| ImageError::Decode(format!("truncated OS/2 BMP {field}")))
}

fn decode_os2_core_bmp(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    const FILE_HEADER: usize = 14;
    const CORE_HEADER: usize = 12;
    const HEADER_END: usize = FILE_HEADER + CORE_HEADER;

    if bytes.len() < HEADER_END {
        return Err(ImageError::Decode("truncated OS/2 BMP core header".into()));
    }
    let pixel_offset = u32_le(bytes, 10, "pixel offset")? as usize;
    if pixel_offset < HEADER_END || pixel_offset > bytes.len() {
        return Err(ImageError::Decode("invalid OS/2 BMP pixel offset".into()));
    }

    let width = u16_le(bytes, 18, "width")? as usize;
    let height = u16_le(bytes, 20, "height")? as usize;
    if width == 0 || height == 0 {
        return Err(ImageError::Decode("OS/2 BMP dimensions must be non-zero".into()));
    }
    if u16_le(bytes, 22, "planes")? != 1 {
        return Err(ImageError::Decode("OS/2 BMP requires one plane".into()));
    }
    let depth = u16_le(bytes, 24, "bits per pixel")?;
    if !matches!(depth, 1 | 4 | 8 | 24) {
        return Err(ImageError::Decode(format!(
            "unsupported OS/2 BMP bit depth {depth}"
        )));
    }

    let palette_entries = match depth {
        1 => 2usize,
        4 => 16usize,
        8 => 256usize,
        24 => 0usize,
        _ => unreachable!(),
    };
    let palette_len = palette_entries
        .checked_mul(3)
        .ok_or_else(|| ImageError::Decode("OS/2 BMP palette size overflow".into()))?;
    let palette_end = HEADER_END
        .checked_add(palette_len)
        .ok_or_else(|| ImageError::Decode("OS/2 BMP palette offset overflow".into()))?;
    if palette_end > pixel_offset || palette_end > bytes.len() {
        return Err(ImageError::Decode("truncated OS/2 BMP RGBTRIPLE palette".into()));
    }

    let palette: Vec<[u8; 4]> = (0..palette_entries)
        .map(|index| {
            let base = HEADER_END + index * 3;
            [bytes[base + 2], bytes[base + 1], bytes[base], 255]
        })
        .collect();

    let row_bits = width
        .checked_mul(depth as usize)
        .ok_or_else(|| ImageError::Decode("OS/2 BMP row bit count overflow".into()))?;
    let row_payload = row_bits
        .checked_add(7)
        .map(|n| n / 8)
        .ok_or_else(|| ImageError::Decode("OS/2 BMP row payload overflow".into()))?;
    let row_stride = row_payload
        .checked_add(3)
        .map(|n| n & !3)
        .ok_or_else(|| ImageError::Decode("OS/2 BMP row padding overflow".into()))?;
    let raster_len = row_stride
        .checked_mul(height)
        .ok_or_else(|| ImageError::Decode("OS/2 BMP raster size overflow".into()))?;
    let raster_end = pixel_offset
        .checked_add(raster_len)
        .ok_or_else(|| ImageError::Decode("OS/2 BMP raster offset overflow".into()))?;
    if raster_end > bytes.len() {
        return Err(ImageError::Decode("truncated OS/2 BMP raster".into()));
    }

    let rgba_len = width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| ImageError::Decode("OS/2 BMP RGBA size overflow".into()))?;
    let mut pixels = vec![0u8; rgba_len];

    for file_y in 0..height {
        let image_y = height - 1 - file_y;
        let row = pixel_offset + file_y * row_stride;
        for x in 0..width {
            let rgba = match depth {
                1 => {
                    let byte = bytes[row + x / 8];
                    let index = ((byte >> (7 - (x % 8))) & 1) as usize;
                    palette[index]
                }
                4 => {
                    let byte = bytes[row + x / 2];
                    let index = if x % 2 == 0 { byte >> 4 } else { byte & 0x0f } as usize;
                    palette[index]
                }
                8 => palette[bytes[row + x] as usize],
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

    Ok(RasterImage::new(width as u32, height as u32, pixels))
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

    fn core_bmp(width: u16, height: u16, depth: u16, palette: &[[u8; 3]], raster: &[u8]) -> Vec<u8> {
        let offset = 26 + palette.len() * 3;
        let mut out = vec![0u8; offset];
        out[0..2].copy_from_slice(b"BM");
        out[2..6].copy_from_slice(&((offset + raster.len()) as u32).to_le_bytes());
        out[10..14].copy_from_slice(&(offset as u32).to_le_bytes());
        out[14..18].copy_from_slice(&12u32.to_le_bytes());
        out[18..20].copy_from_slice(&width.to_le_bytes());
        out[20..22].copy_from_slice(&height.to_le_bytes());
        out[22..24].copy_from_slice(&1u16.to_le_bytes());
        out[24..26].copy_from_slice(&depth.to_le_bytes());
        for (i, rgb) in palette.iter().enumerate() {
            let base = 26 + i * 3;
            out[base..base + 3].copy_from_slice(&[rgb[2], rgb[1], rgb[0]]);
        }
        out.extend_from_slice(raster);
        out
    }

    fn palette(size: usize) -> Vec<[u8; 3]> {
        let mut entries = vec![[0, 0, 0]; size];
        if size > 1 { entries[1] = [255, 0, 0]; }
        if size > 2 { entries[2] = [0, 255, 0]; }
        entries
    }

    #[test]
    fn decodes_24_bit_bottom_up_with_padding() {
        let bytes = core_bmp(
            1,
            2,
            24,
            &[],
            &[
                255, 0, 0, 0, // bottom: blue + pad
                0, 0, 255, 0, // top: red + pad
            ],
        );
        let image = decode(&bytes).expect("OS/2 24-bit BMP");
        assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(image.pixel(0, 1), [0, 0, 255, 255]);
    }

    #[test]
    fn decodes_rgbtriple_palette_for_1_4_and_8_bit() {
        let p1 = palette(2);
        let one = core_bmp(2, 1, 1, &p1, &[0b0100_0000, 0, 0, 0]);
        let image = decode(&one).expect("1-bit core BMP");
        assert_eq!(image.pixel(0, 0), [0, 0, 0, 255]);
        assert_eq!(image.pixel(1, 0), [255, 0, 0, 255]);

        let p4 = palette(16);
        let four = core_bmp(2, 1, 4, &p4, &[0x12, 0, 0, 0]);
        let image = decode(&four).expect("4-bit core BMP");
        assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(image.pixel(1, 0), [0, 255, 0, 255]);

        let p8 = palette(256);
        let eight = core_bmp(2, 1, 8, &p8, &[2, 1, 0, 0]);
        let image = decode(&eight).expect("8-bit core BMP");
        assert_eq!(image.pixel(0, 0), [0, 255, 0, 255]);
        assert_eq!(image.pixel(1, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn rejects_bad_core_headers_and_truncation() {
        let p = palette(2);
        assert!(decode(&core_bmp(0, 1, 1, &p, &[0; 4])).is_err());
        assert!(decode(&core_bmp(1, 1, 2, &[], &[0; 4])).is_err());
        let mut truncated = core_bmp(1, 1, 1, &p, &[0; 4]);
        truncated.truncate(truncated.len() - 1);
        assert!(decode(&truncated).is_err());
    }
}
