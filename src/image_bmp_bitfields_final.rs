// ============================================================
// image_bmp_bitfields_final.rs — Windows BMP bitfield facade
// ============================================================

use std::collections::HashMap;
use std::rc::Rc;

use crate::net::{LoadError, ResourceLoader, Url};

pub use crate::image_prev9::{ImageError, ImageFormat, RasterImage};

/// Decode image bytes into straight RGBA8, adding BI_BITFIELDS support on top
/// of the existing BMP/image decoder stack.
pub fn decode(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    if is_bitfields_bmp(bytes) {
        decode_bitfields_bmp(bytes)
    } else {
        crate::image_prev9::decode(bytes)
    }
}

fn is_bitfields_bmp(bytes: &[u8]) -> bool {
    bytes.starts_with(b"BM") && read_u32_raw(bytes, 30) == Some(3)
}

fn read_u16_raw(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(offset..offset + 2)?.try_into().ok()?))
}

fn read_u32_raw(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(offset..offset + 4)?.try_into().ok()?))
}

fn u16_le(bytes: &[u8], offset: usize, field: &str) -> Result<u16, ImageError> {
    read_u16_raw(bytes, offset)
        .ok_or_else(|| ImageError::Decode(format!("truncated BMP {field}")))
}

fn u32_le(bytes: &[u8], offset: usize, field: &str) -> Result<u32, ImageError> {
    read_u32_raw(bytes, offset)
        .ok_or_else(|| ImageError::Decode(format!("truncated BMP {field}")))
}

fn i32_le(bytes: &[u8], offset: usize, field: &str) -> Result<i32, ImageError> {
    Ok(u32_le(bytes, offset, field)? as i32)
}

#[derive(Clone, Copy)]
struct ChannelMask {
    mask: u32,
    shift: u32,
    max: u32,
}

impl ChannelMask {
    fn parse(mask: u32, depth: u16, name: &str) -> Result<Self, ImageError> {
        if mask == 0 {
            return Err(ImageError::Decode(format!("BMP {name} bitfield mask is zero")));
        }
        if depth < 32 && mask >= (1u32 << depth) {
            return Err(ImageError::Decode(format!("BMP {name} bitfield mask exceeds pixel depth")));
        }
        let shift = mask.trailing_zeros();
        let normalized = mask >> shift;
        // Windows bitfield masks describe one contiguous run of channel bits.
        if normalized & normalized.wrapping_add(1) != 0 {
            return Err(ImageError::Decode(format!("BMP {name} bitfield mask is non-contiguous")));
        }
        Ok(Self { mask, shift, max: normalized })
    }

    fn byte(self, pixel: u32) -> u8 {
        let value = (pixel & self.mask) >> self.shift;
        (((value as u64) * 255 + (self.max as u64 / 2)) / self.max as u64) as u8
    }
}

fn decode_bitfields_bmp(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    if bytes.len() < 66 {
        return Err(ImageError::Decode("truncated BMP BI_BITFIELDS header".into()));
    }
    let pixel_offset = u32_le(bytes, 10, "pixel offset")? as usize;
    let dib_size = u32_le(bytes, 14, "DIB header size")? as usize;
    if dib_size < 40 {
        return Err(ImageError::Decode(format!("unsupported BMP DIB header size {dib_size}")));
    }
    let dib_end = 14usize.checked_add(dib_size)
        .ok_or_else(|| ImageError::Decode("BMP DIB header size overflow".into()))?;
    if dib_end > bytes.len() || pixel_offset > bytes.len() {
        return Err(ImageError::Decode("invalid BMP header/pixel offset".into()));
    }

    let width_signed = i32_le(bytes, 18, "width")?;
    let height_signed = i32_le(bytes, 22, "height")?;
    if width_signed <= 0 || height_signed == 0 {
        return Err(ImageError::Decode("BMP bitfields require positive width and non-zero height".into()));
    }
    if u16_le(bytes, 26, "planes")? != 1 {
        return Err(ImageError::Decode("BI_BITFIELDS requires one plane".into()));
    }
    let depth = u16_le(bytes, 28, "bits per pixel")?;
    if depth != 16 && depth != 32 {
        return Err(ImageError::Decode("BI_BITFIELDS supports only 16- or 32-bit pixels".into()));
    }
    if u32_le(bytes, 30, "compression")? != 3 {
        return Err(ImageError::Decode("BMP is not BI_BITFIELDS-compressed".into()));
    }

    // BITMAPINFOHEADER stores the masks immediately after its 40-byte body;
    // V2+ headers store the same fields at these fixed offsets inside the DIB.
    let masks_end = 66usize;
    if pixel_offset < masks_end || bytes.len() < masks_end {
        return Err(ImageError::Decode("truncated BMP BI_BITFIELDS masks".into()));
    }
    let red_raw = u32_le(bytes, 54, "red mask")?;
    let green_raw = u32_le(bytes, 58, "green mask")?;
    let blue_raw = u32_le(bytes, 62, "blue mask")?;
    if (red_raw & green_raw) != 0 || (red_raw & blue_raw) != 0 || (green_raw & blue_raw) != 0 {
        return Err(ImageError::Decode("BMP BI_BITFIELDS channel masks overlap".into()));
    }
    let red = ChannelMask::parse(red_raw, depth, "red")?;
    let green = ChannelMask::parse(green_raw, depth, "green")?;
    let blue = ChannelMask::parse(blue_raw, depth, "blue")?;

    let width = width_signed as usize;
    let height = height_signed.unsigned_abs() as usize;
    let bytes_per_pixel = (depth / 8) as usize;
    let row_payload = width.checked_mul(bytes_per_pixel)
        .ok_or_else(|| ImageError::Decode("BMP bitfields row size overflow".into()))?;
    let row_stride = row_payload.checked_add(3)
        .map(|n| n & !3)
        .ok_or_else(|| ImageError::Decode("BMP bitfields row padding overflow".into()))?;
    let raster_len = row_stride.checked_mul(height)
        .ok_or_else(|| ImageError::Decode("BMP bitfields raster size overflow".into()))?;
    let raster_end = pixel_offset.checked_add(raster_len)
        .ok_or_else(|| ImageError::Decode("BMP bitfields raster offset overflow".into()))?;
    if raster_end > bytes.len() {
        return Err(ImageError::Decode("truncated BMP BI_BITFIELDS raster".into()));
    }
    let capacity = width.checked_mul(height).and_then(|n| n.checked_mul(4))
        .ok_or_else(|| ImageError::Decode("BMP bitfields RGBA size overflow".into()))?;
    let mut pixels = vec![0u8; capacity];
    let top_down = height_signed < 0;

    for file_y in 0..height {
        let image_y = if top_down { file_y } else { height - 1 - file_y };
        let row = pixel_offset + file_y * row_stride;
        for x in 0..width {
            let src = row + x * bytes_per_pixel;
            let pixel = if depth == 16 {
                u16::from_le_bytes(bytes[src..src + 2].try_into().expect("raster bounds checked")) as u32
            } else {
                u32::from_le_bytes(bytes[src..src + 4].try_into().expect("raster bounds checked"))
            };
            let dst = (image_y * width + x) * 4;
            pixels[dst..dst + 4].copy_from_slice(&[red.byte(pixel), green.byte(pixel), blue.byte(pixel), 255]);
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

    fn bmp(width: i32, height: i32, depth: u16, masks: [u32; 3], raster: &[u8]) -> Vec<u8> {
        let offset = 66usize;
        let mut out = vec![0u8; offset];
        out[0..2].copy_from_slice(b"BM");
        out[2..6].copy_from_slice(&((offset + raster.len()) as u32).to_le_bytes());
        out[10..14].copy_from_slice(&(offset as u32).to_le_bytes());
        out[14..18].copy_from_slice(&40u32.to_le_bytes());
        out[18..22].copy_from_slice(&width.to_le_bytes());
        out[22..26].copy_from_slice(&height.to_le_bytes());
        out[26..28].copy_from_slice(&1u16.to_le_bytes());
        out[28..30].copy_from_slice(&depth.to_le_bytes());
        out[30..34].copy_from_slice(&3u32.to_le_bytes());
        for (i, mask) in masks.into_iter().enumerate() {
            out[54 + i * 4..58 + i * 4].copy_from_slice(&mask.to_le_bytes());
        }
        out.extend_from_slice(raster);
        out
    }

    #[test]
    fn decodes_rgb565_and_bottom_up_rows() {
        let bytes = bmp(2, 1, 16, [0xf800, 0x07e0, 0x001f], &[0x00, 0xf8, 0x1f, 0x00]);
        let image = decode(&bytes).expect("RGB565");
        assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(image.pixel(1, 0), [0, 0, 255, 255]);
    }

    #[test]
    fn decodes_top_down_32_bit_masks() {
        let bytes = bmp(1, -1, 32, [0x00ff0000, 0x0000ff00, 0x000000ff], &[0x33, 0x22, 0x11, 0xaa]);
        let image = decode(&bytes).expect("32-bit bitfields");
        assert_eq!(image.pixel(0, 0), [0x11, 0x22, 0x33, 255]);
    }

    #[test]
    fn rejects_overlapping_or_noncontiguous_masks() {
        assert!(decode(&bmp(1, 1, 16, [0xf800, 0x7800, 0x001f], &[0, 0, 0, 0])).is_err());
        assert!(decode(&bmp(1, 1, 16, [0xa800, 0x0700, 0x001f], &[0, 0, 0, 0])).is_err());
    }
}
