// ============================================================
// image_ico_bitfields_final.rs — masked-channel ICO DIB facade
// ============================================================

use std::collections::HashMap;
use std::rc::Rc;

use crate::net::{LoadError, ResourceLoader, Url};

pub use crate::image_prev15::{ImageError, ImageFormat, RasterImage};

pub fn decode(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    if looks_like_ico(bytes) {
        decode_ico(bytes)
    } else {
        crate::image_prev15::decode(bytes)
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

fn read_i32(bytes: &[u8], offset: usize, field: &str) -> Result<i32, ImageError> {
    let raw = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| ImageError::Decode(format!("truncated ICO {field}")))?;
    Ok(i32::from_le_bytes(raw.try_into().unwrap()))
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
        .checked_add(
            count
                .checked_mul(16)
                .ok_or_else(|| ImageError::Decode("ICO directory size overflow".into()))?,
        )
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
            return Err(ImageError::Decode(format!(
                "ICO entry {index} reserved byte must be zero"
            )));
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
            return Err(ImageError::Decode(format!(
                "ICO entry {index} payload is out of bounds"
            )));
        }
        entries.push(Entry {
            width,
            height,
            bit_depth,
            size,
            offset,
            ordinal: index,
        });
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
    if payload.starts_with(b"\x89PNG\r\n\x1a\n") {
        return crate::image_prev15::decode(bytes);
    }
    if payload.len() < 20 {
        return crate::image_prev15::decode(bytes);
    }
    let compression = read_u32(payload, 16, "DIB compression")?;
    if compression != 3 && compression != 6 {
        return crate::image_prev15::decode(bytes);
    }
    decode_masked_dib(payload, selected, compression)
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
            return Err(ImageError::Decode(format!("ICO DIB {name} mask must be non-zero")));
        }
        if depth < 32 && mask >= (1u32 << depth) {
            return Err(ImageError::Decode(format!(
                "ICO DIB {name} mask exceeds {depth}-bit pixel depth"
            )));
        }
        let shift = mask.trailing_zeros();
        let shifted = mask >> shift;
        if shifted & shifted.wrapping_add(1) != 0 {
            return Err(ImageError::Decode(format!(
                "ICO DIB {name} mask must contain contiguous bits"
            )));
        }
        Ok(Self {
            mask,
            shift,
            max: shifted,
        })
    }

    fn extract(self, pixel: u32) -> u8 {
        let raw = (pixel & self.mask) >> self.shift;
        ((raw as u64 * 255 + (self.max as u64 / 2)) / self.max as u64) as u8
    }
}

fn read_masks(
    payload: &[u8],
    header_size: usize,
    bit_depth: u16,
    compression: u32,
) -> Result<(ChannelMask, ChannelMask, ChannelMask, Option<ChannelMask>, usize), ImageError> {
    let external = header_size == 40;
    let mask_start = if external { 40 } else { 40 };
    if !external && header_size < 52 {
        return Err(ImageError::Decode(
            "ICO BI_BITFIELDS DIB header is too small to contain RGB masks".into(),
        ));
    }
    let r_raw = read_u32(payload, mask_start, "DIB red mask")?;
    let g_raw = read_u32(payload, mask_start + 4, "DIB green mask")?;
    let b_raw = read_u32(payload, mask_start + 8, "DIB blue mask")?;
    let r = ChannelMask::parse(r_raw, bit_depth, "red")?;
    let g = ChannelMask::parse(g_raw, bit_depth, "green")?;
    let b = ChannelMask::parse(b_raw, bit_depth, "blue")?;
    if r.mask & g.mask != 0 || r.mask & b.mask != 0 || g.mask & b.mask != 0 {
        return Err(ImageError::Decode("ICO DIB RGB masks must not overlap".into()));
    }

    let mut external_mask_bytes = if external { 12 } else { 0 };
    let alpha = if compression == 6 {
        if bit_depth != 32 {
            return Err(ImageError::Decode(
                "ICO BI_ALPHABITFIELDS currently requires 32-bit pixels".into(),
            ));
        }
        let alpha_offset = if external {
            external_mask_bytes = 16;
            52
        } else {
            if header_size < 56 {
                return Err(ImageError::Decode(
                    "ICO BI_ALPHABITFIELDS DIB header is too small for an alpha mask".into(),
                ));
            }
            52
        };
        let a = ChannelMask::parse(read_u32(payload, alpha_offset, "DIB alpha mask")?, bit_depth, "alpha")?;
        if a.mask & (r.mask | g.mask | b.mask) != 0 {
            return Err(ImageError::Decode(
                "ICO DIB alpha mask must not overlap RGB masks".into(),
            ));
        }
        Some(a)
    } else {
        None
    };

    Ok((r, g, b, alpha, external_mask_bytes))
}

fn dib_stride(width: u32, bits_per_pixel: u16, what: &str) -> Result<usize, ImageError> {
    let bits = usize::try_from(width)
        .map_err(|_| ImageError::Decode(format!("ICO {what} width does not fit this platform")))?
        .checked_mul(bits_per_pixel as usize)
        .ok_or_else(|| ImageError::Decode(format!("ICO {what} row size overflow")))?;
    let dwords = bits
        .checked_add(31)
        .ok_or_else(|| ImageError::Decode(format!("ICO {what} row size overflow")))?
        / 32;
    dwords
        .checked_mul(4)
        .ok_or_else(|| ImageError::Decode(format!("ICO {what} row size overflow")))
}

fn decode_masked_dib(payload: &[u8], entry: Entry, compression: u32) -> Result<RasterImage, ImageError> {
    let header_size = usize::try_from(read_u32(payload, 0, "DIB header size")?)
        .map_err(|_| ImageError::Decode("ICO DIB header size does not fit this platform".into()))?;
    if header_size < 40 || header_size > payload.len() {
        return Err(ImageError::Decode(format!(
            "unsupported or truncated ICO DIB header size {header_size}"
        )));
    }
    let dib_width = read_i32(payload, 4, "DIB width")?;
    let stored_height = read_i32(payload, 8, "DIB height")?;
    if dib_width <= 0 || stored_height <= 0 || stored_height % 2 != 0 {
        return Err(ImageError::Decode(
            "ICO DIB dimensions must be positive and stored height must contain XOR+AND rows".into(),
        ));
    }
    let width = dib_width as u32;
    let height = (stored_height / 2) as u32;
    if width != entry.width || height != entry.height {
        return Err(ImageError::Decode(format!(
            "ICO directory dimensions {}x{} do not match DIB payload {}x{}",
            entry.width, entry.height, width, height
        )));
    }
    if read_u16(payload, 12, "DIB planes")? != 1 {
        return Err(ImageError::Decode("ICO DIB must have one color plane".into()));
    }
    let bit_depth = read_u16(payload, 14, "DIB bit depth")?;
    if !matches!(bit_depth, 16 | 32) {
        return Err(ImageError::Decode(format!(
            "ICO BI_BITFIELDS supports 16/32-bit pixels, found {bit_depth}"
        )));
    }
    if entry.bit_depth != 0 && entry.bit_depth != bit_depth {
        return Err(ImageError::Decode(format!(
            "ICO directory bit depth {} does not match DIB payload {bit_depth}",
            entry.bit_depth
        )));
    }

    let (red, green, blue, alpha, external_mask_bytes) =
        read_masks(payload, header_size, bit_depth, compression)?;
    let xor_start = header_size
        .checked_add(external_mask_bytes)
        .ok_or_else(|| ImageError::Decode("ICO DIB pixel offset overflow".into()))?;
    if xor_start > payload.len() {
        return Err(ImageError::Decode("truncated ICO DIB channel masks".into()));
    }
    let height_usize = height as usize;
    let width_usize = width as usize;
    let xor_stride = dib_stride(width, bit_depth, "XOR")?;
    let xor_bytes = xor_stride
        .checked_mul(height_usize)
        .ok_or_else(|| ImageError::Decode("ICO DIB XOR bitmap size overflow".into()))?;
    let xor_end = xor_start
        .checked_add(xor_bytes)
        .ok_or_else(|| ImageError::Decode("ICO DIB XOR bitmap range overflow".into()))?;
    if xor_end > payload.len() {
        return Err(ImageError::Decode("truncated ICO DIB XOR bitmap".into()));
    }
    let and_stride = dib_stride(width, 1, "AND mask")?;
    let and_bytes = and_stride
        .checked_mul(height_usize)
        .ok_or_else(|| ImageError::Decode("ICO DIB AND mask size overflow".into()))?;
    let and_end = xor_end
        .checked_add(and_bytes)
        .ok_or_else(|| ImageError::Decode("ICO DIB AND mask range overflow".into()))?;
    if and_end > payload.len() {
        return Err(ImageError::Decode("missing or truncated ICO DIB AND mask".into()));
    }

    let mut pixels = vec![0u8; width_usize
        .checked_mul(height_usize)
        .and_then(|count| count.checked_mul(4))
        .ok_or_else(|| ImageError::Decode("ICO DIB RGBA size overflow".into()))?];
    for y in 0..height_usize {
        let source_y = height_usize - 1 - y;
        let row_start = xor_start + source_y * xor_stride;
        let row = &payload[row_start..row_start + xor_stride];
        for x in 0..width_usize {
            let pixel = if bit_depth == 16 {
                let base = x * 2;
                u16::from_le_bytes([row[base], row[base + 1]]) as u32
            } else {
                let base = x * 4;
                u32::from_le_bytes([row[base], row[base + 1], row[base + 2], row[base + 3]])
            };
            let dst = (y * width_usize + x) * 4;
            pixels[dst] = red.extract(pixel);
            pixels[dst + 1] = green.extract(pixel);
            pixels[dst + 2] = blue.extract(pixel);
            pixels[dst + 3] = alpha.map(|mask| mask.extract(pixel)).unwrap_or(255);
        }
    }

    // Explicit alpha masks carry authored transparency. Plain BI_BITFIELDS has
    // only RGB masks, so retain the ICO AND mask as its binary alpha plane.
    if alpha.is_none() {
        for y in 0..height_usize {
            let source_y = height_usize - 1 - y;
            let row_start = xor_end + source_y * and_stride;
            let row = &payload[row_start..row_start + and_stride];
            for x in 0..width_usize {
                if ((row[x / 8] >> (7 - (x % 8))) & 1) != 0 {
                    pixels[(y * width_usize + x) * 4 + 3] = 0;
                }
            }
        }
    }

    Ok(RasterImage::new(width, height, pixels))
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

    fn masked_ico(bit_depth: u16, compression: u32, masks: &[u32], pixel: u32, and_mask: u8) -> Vec<u8> {
        let width = 1u8;
        let height = 1u8;
        let external_masks = if masks.is_empty() { 0 } else { masks.len() * 4 };
        let xor_stride = 4usize;
        let mut dib = vec![0u8; 40 + external_masks];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&1i32.to_le_bytes());
        dib[8..12].copy_from_slice(&2i32.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&bit_depth.to_le_bytes());
        dib[16..20].copy_from_slice(&compression.to_le_bytes());
        for (i, mask) in masks.iter().enumerate() {
            dib[40 + i * 4..44 + i * 4].copy_from_slice(&mask.to_le_bytes());
        }
        if bit_depth == 16 {
            dib.extend_from_slice(&(pixel as u16).to_le_bytes());
            dib.resize(dib.len() + (xor_stride - 2), 0);
        } else {
            dib.extend_from_slice(&pixel.to_le_bytes());
        }
        dib.extend_from_slice(&[and_mask, 0, 0, 0]);

        let mut out = vec![0u8; 22];
        out[2..4].copy_from_slice(&1u16.to_le_bytes());
        out[4..6].copy_from_slice(&1u16.to_le_bytes());
        out[6] = width;
        out[7] = height;
        out[10..12].copy_from_slice(&1u16.to_le_bytes());
        out[12..14].copy_from_slice(&bit_depth.to_le_bytes());
        out[14..18].copy_from_slice(&(dib.len() as u32).to_le_bytes());
        out[18..22].copy_from_slice(&22u32.to_le_bytes());
        out.extend_from_slice(&dib);
        out
    }

    #[test]
    fn decodes_rgb565_bitfields_and_and_mask() {
        let bytes = masked_ico(16, 3, &[0xf800, 0x07e0, 0x001f], 0xf800, 0);
        let image = decode(&bytes).unwrap();
        assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);

        let transparent = masked_ico(16, 3, &[0xf800, 0x07e0, 0x001f], 0x07e0, 0x80);
        assert_eq!(decode(&transparent).unwrap().pixel(0, 0), [0, 255, 0, 0]);
    }

    #[test]
    fn decodes_32bit_alpha_bitfields() {
        let pixel = 0x80402010u32;
        let bytes = masked_ico(
            32,
            6,
            &[0x00ff0000, 0x0000ff00, 0x000000ff, 0xff000000],
            pixel,
            0x80,
        );
        assert_eq!(decode(&bytes).unwrap().pixel(0, 0), [64, 32, 16, 128]);
    }

    #[test]
    fn rejects_overlapping_and_noncontiguous_masks() {
        let overlapping = masked_ico(16, 3, &[0x7c00, 0x7c00, 0x001f], 0, 0);
        assert!(decode(&overlapping).is_err());

        let noncontiguous = masked_ico(16, 3, &[0x7400, 0x03e0, 0x001f], 0, 0);
        assert!(decode(&noncontiguous).is_err());
    }
}
