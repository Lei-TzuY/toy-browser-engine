// ============================================================
//  image_bmp_final.rs — Windows BMP image facade
// ============================================================

use std::collections::HashMap;
use std::rc::Rc;

use crate::net::{LoadError, ResourceLoader, Url};

pub use crate::image_prev6::{ImageError, ImageFormat, RasterImage};

/// Decode image bytes into straight RGBA8, adding uncompressed Windows BMP
/// support on top of the existing PNG/JPEG/PNM/PAM/PFM stack.
pub fn decode(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    if bytes.starts_with(b"BM") {
        decode_bmp(bytes)
    } else {
        crate::image_prev6::decode(bytes)
    }
}

fn u16_le(bytes: &[u8], offset: usize, field: &str) -> Result<u16, ImageError> {
    let end = offset
        .checked_add(2)
        .ok_or_else(|| ImageError::Decode(format!("BMP {field} offset overflow")))?;
    let raw: [u8; 2] = bytes
        .get(offset..end)
        .ok_or_else(|| ImageError::Decode(format!("truncated BMP {field}")))?
        .try_into()
        .expect("slice length checked");
    Ok(u16::from_le_bytes(raw))
}

fn u32_le(bytes: &[u8], offset: usize, field: &str) -> Result<u32, ImageError> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| ImageError::Decode(format!("BMP {field} offset overflow")))?;
    let raw: [u8; 4] = bytes
        .get(offset..end)
        .ok_or_else(|| ImageError::Decode(format!("truncated BMP {field}")))?
        .try_into()
        .expect("slice length checked");
    Ok(u32::from_le_bytes(raw))
}

fn i32_le(bytes: &[u8], offset: usize, field: &str) -> Result<i32, ImageError> {
    Ok(u32_le(bytes, offset, field)? as i32)
}

fn decode_bmp(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    if bytes.len() < 14 {
        return Err(ImageError::Decode("truncated BMP file header".into()));
    }

    let pixel_offset = u32_le(bytes, 10, "pixel offset")? as usize;
    let dib_size = u32_le(bytes, 14, "DIB header size")? as usize;
    if dib_size < 40 {
        return Err(ImageError::Decode(format!(
            "unsupported BMP DIB header size {dib_size}"
        )));
    }
    let dib_end = 14usize
        .checked_add(dib_size)
        .ok_or_else(|| ImageError::Decode("BMP DIB header size overflow".into()))?;
    if dib_end > bytes.len() {
        return Err(ImageError::Decode("truncated BMP DIB header".into()));
    }
    if pixel_offset < dib_end {
        return Err(ImageError::Decode(
            "BMP pixel array overlaps the DIB header".into(),
        ));
    }

    let width_signed = i32_le(bytes, 18, "width")?;
    let height_signed = i32_le(bytes, 22, "height")?;
    if width_signed <= 0 {
        return Err(ImageError::Decode("BMP width must be positive".into()));
    }
    if height_signed == 0 || height_signed == i32::MIN {
        return Err(ImageError::Decode("BMP height must be non-zero".into()));
    }

    let planes = u16_le(bytes, 26, "planes")?;
    if planes != 1 {
        return Err(ImageError::Decode(format!(
            "unsupported BMP plane count {planes}"
        )));
    }
    let bits_per_pixel = u16_le(bytes, 28, "bits per pixel")?;
    if !matches!(bits_per_pixel, 1 | 4 | 8 | 24 | 32) {
        return Err(ImageError::Decode(format!(
            "unsupported BMP bit depth {bits_per_pixel}"
        )));
    }
    let compression = u32_le(bytes, 30, "compression")?;
    if compression != 0 {
        return Err(ImageError::Decode(format!(
            "unsupported BMP compression {compression}"
        )));
    }

    let palette = if bits_per_pixel <= 8 {
        let max_palette_len = 1usize << bits_per_pixel;
        let colors_used = u32_le(bytes, 46, "colors used")?;
        let palette_len = if colors_used == 0 {
            max_palette_len
        } else {
            usize::try_from(colors_used)
                .map_err(|_| ImageError::Decode("BMP palette size overflow".into()))?
        };
        if palette_len == 0 || palette_len > max_palette_len {
            return Err(ImageError::Decode(format!(
                "invalid BMP palette size {palette_len} for {bits_per_pixel}-bit pixels"
            )));
        }
        let palette_bytes = palette_len
            .checked_mul(4)
            .ok_or_else(|| ImageError::Decode("BMP palette size overflow".into()))?;
        let palette_end = dib_end
            .checked_add(palette_bytes)
            .ok_or_else(|| ImageError::Decode("BMP palette offset overflow".into()))?;
        if palette_end > pixel_offset || palette_end > bytes.len() {
            return Err(ImageError::Decode("truncated BMP color palette".into()));
        }
        Some(&bytes[dib_end..palette_end])
    } else {
        None
    };

    let width = width_signed as u32;
    let height = height_signed.unsigned_abs();
    let row_payload = match bits_per_pixel {
        1 | 4 | 8 => {
            let row_bits = (width as usize)
                .checked_mul(bits_per_pixel as usize)
                .ok_or_else(|| ImageError::Decode("BMP row size overflow".into()))?;
            row_bits
                .checked_add(7)
                .map(|bits| bits / 8)
                .ok_or_else(|| ImageError::Decode("BMP row size overflow".into()))?
        }
        24 | 32 => (width as usize)
            .checked_mul((bits_per_pixel / 8) as usize)
            .ok_or_else(|| ImageError::Decode("BMP row size overflow".into()))?,
        _ => unreachable!("bit depth validated above"),
    };
    let row_stride = row_payload
        .checked_add(3)
        .map(|n| n & !3)
        .ok_or_else(|| ImageError::Decode("BMP row stride overflow".into()))?;
    let raster_bytes = row_stride
        .checked_mul(height as usize)
        .ok_or_else(|| ImageError::Decode("BMP raster size overflow".into()))?;
    let raster_end = pixel_offset
        .checked_add(raster_bytes)
        .ok_or_else(|| ImageError::Decode("BMP raster offset overflow".into()))?;
    if raster_end > bytes.len() {
        return Err(ImageError::Decode("truncated BMP pixel array".into()));
    }

    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| ImageError::Decode("BMP dimensions overflow pixel count".into()))?;
    let capacity = pixel_count
        .checked_mul(4)
        .ok_or_else(|| ImageError::Decode("BMP dimensions overflow RGBA buffer".into()))?;
    let mut pixels = vec![0u8; capacity];
    let top_down = height_signed < 0;

    for file_y in 0..height as usize {
        let image_y = if top_down {
            file_y
        } else {
            height as usize - 1 - file_y
        };
        let row_start = pixel_offset + file_y * row_stride;
        let row = &bytes[row_start..row_start + row_payload];
        for x in 0..width as usize {
            let dst = (image_y * width as usize + x) * 4;
            match bits_per_pixel {
                1 | 4 | 8 => {
                    let palette = palette.expect("indexed BMP palette validated above");
                    let index = match bits_per_pixel {
                        1 => {
                            let byte = row[x / 8];
                            ((byte >> (7 - (x % 8))) & 0x01) as usize
                        }
                        4 => {
                            let byte = row[x / 2];
                            if x % 2 == 0 {
                                (byte >> 4) as usize
                            } else {
                                (byte & 0x0f) as usize
                            }
                        }
                        8 => row[x] as usize,
                        _ => unreachable!(),
                    };
                    let src = index.checked_mul(4).ok_or_else(|| {
                        ImageError::Decode("BMP palette index overflow".into())
                    })?;
                    let entry = palette.get(src..src + 4).ok_or_else(|| {
                        ImageError::Decode(format!(
                            "BMP palette index {index} exceeds declared palette"
                        ))
                    })?;
                    // BITMAPINFO RGBQUAD entries are B, G, R, reserved.
                    pixels[dst] = entry[2];
                    pixels[dst + 1] = entry[1];
                    pixels[dst + 2] = entry[0];
                    pixels[dst + 3] = 255;
                }
                24 | 32 => {
                    let bytes_per_pixel = (bits_per_pixel / 8) as usize;
                    let src = x * bytes_per_pixel;
                    // BI_RGB pixels are stored BGR/BGRX. For 32-bit BI_RGB the fourth
                    // byte is reserved rather than a reliable alpha channel.
                    pixels[dst] = row[src + 2];
                    pixels[dst + 1] = row[src + 1];
                    pixels[dst + 2] = row[src];
                    pixels[dst + 3] = 255;
                }
                _ => unreachable!("bit depth validated above"),
            }
        }
    }

    Ok(RasterImage::new(width, height, pixels))
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

    fn bmp(width: i32, height: i32, bpp: u16, rows: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; 54];
        out[0..2].copy_from_slice(b"BM");
        let size = 54u32 + rows.len() as u32;
        out[2..6].copy_from_slice(&size.to_le_bytes());
        out[10..14].copy_from_slice(&54u32.to_le_bytes());
        out[14..18].copy_from_slice(&40u32.to_le_bytes());
        out[18..22].copy_from_slice(&width.to_le_bytes());
        out[22..26].copy_from_slice(&height.to_le_bytes());
        out[26..28].copy_from_slice(&1u16.to_le_bytes());
        out[28..30].copy_from_slice(&bpp.to_le_bytes());
        out.extend_from_slice(rows);
        out
    }

    fn indexed_bmp(
        width: i32,
        height: i32,
        bpp: u16,
        palette: &[[u8; 4]],
        rows: &[u8],
    ) -> Vec<u8> {
        let pixel_offset = 54 + palette.len() * 4;
        let mut out = vec![0u8; 54];
        out[0..2].copy_from_slice(b"BM");
        let size = pixel_offset + rows.len();
        out[2..6].copy_from_slice(&(size as u32).to_le_bytes());
        out[10..14].copy_from_slice(&(pixel_offset as u32).to_le_bytes());
        out[14..18].copy_from_slice(&40u32.to_le_bytes());
        out[18..22].copy_from_slice(&width.to_le_bytes());
        out[22..26].copy_from_slice(&height.to_le_bytes());
        out[26..28].copy_from_slice(&1u16.to_le_bytes());
        out[28..30].copy_from_slice(&bpp.to_le_bytes());
        out[46..50].copy_from_slice(&(palette.len() as u32).to_le_bytes());
        for entry in palette {
            out.extend_from_slice(entry);
        }
        out.extend_from_slice(rows);
        out
    }

    #[test]
    fn decodes_bottom_up_24_bit_with_row_padding() {
        let bytes = bmp(1, 2, 24, &[0, 0, 255, 0, 0, 255, 0, 0]);
        let image = decode(&bytes).expect("24-bit BMP");
        assert_eq!(image.pixel(0, 0), [0, 255, 0, 255]);
        assert_eq!(image.pixel(0, 1), [255, 0, 0, 255]);
    }

    #[test]
    fn decodes_top_down_32_bit_and_ignores_reserved_alpha_byte() {
        let bytes = bmp(1, -1, 32, &[30, 20, 10, 0]);
        let image = decode(&bytes).expect("32-bit top-down BMP");
        assert_eq!(image.pixel(0, 0), [10, 20, 30, 255]);
    }

    #[test]
    fn decodes_8_bit_palette_and_padding() {
        let palette = [[0, 0, 255, 0], [0, 255, 0, 0], [255, 0, 0, 0]];
        let bytes = indexed_bmp(3, 1, 8, &palette, &[0, 1, 2, 0]);
        let image = decode(&bytes).expect("8-bit indexed BMP");
        assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(image.pixel(1, 0), [0, 255, 0, 255]);
        assert_eq!(image.pixel(2, 0), [0, 0, 255, 255]);
    }

    #[test]
    fn decodes_packed_4_bit_high_nibble_first() {
        let palette = [
            [0, 0, 0, 0],
            [0, 0, 255, 0],
            [0, 255, 0, 0],
            [255, 0, 0, 0],
        ];
        let bytes = indexed_bmp(3, 1, 4, &palette, &[0x12, 0x30, 0, 0]);
        let image = decode(&bytes).expect("4-bit indexed BMP");
        assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(image.pixel(1, 0), [0, 255, 0, 255]);
        assert_eq!(image.pixel(2, 0), [0, 0, 255, 255]);
    }

    #[test]
    fn decodes_packed_1_bit_msb_first_and_ignores_tail_bits() {
        let palette = [[0, 0, 0, 0], [255, 255, 255, 0]];
        let bytes = indexed_bmp(5, 1, 1, &palette, &[0b1010_1111, 0, 0, 0]);
        let image = decode(&bytes).expect("1-bit indexed BMP");
        assert_eq!(image.pixel(0, 0), [255, 255, 255, 255]);
        assert_eq!(image.pixel(1, 0), [0, 0, 0, 255]);
        assert_eq!(image.pixel(2, 0), [255, 255, 255, 255]);
        assert_eq!(image.pixel(3, 0), [0, 0, 0, 255]);
        assert_eq!(image.pixel(4, 0), [255, 255, 255, 255]);
    }

    #[test]
    fn rejects_palette_index_outside_declared_table() {
        let palette = [[0, 0, 0, 0], [255, 255, 255, 0]];
        let bytes = indexed_bmp(1, 1, 8, &palette, &[2, 0, 0, 0]);
        assert!(decode(&bytes).is_err());
    }

    #[test]
    fn rejects_palette_larger_than_bit_depth_can_address() {
        let palette = [[0, 0, 0, 0]; 3];
        let bytes = indexed_bmp(1, 1, 1, &palette, &[0, 0, 0, 0]);
        assert!(decode(&bytes).is_err());
    }

    #[test]
    fn rejects_compression_truncation_and_bad_dimensions() {
        let mut compressed = bmp(1, 1, 24, &[0, 0, 0, 0]);
        compressed[30..34].copy_from_slice(&1u32.to_le_bytes());
        assert!(decode(&compressed).is_err());
        assert!(decode(&bmp(1, 2, 24, &[0, 0, 0, 0])).is_err());
        assert!(decode(&bmp(0, 1, 24, &[])).is_err());
    }
}
