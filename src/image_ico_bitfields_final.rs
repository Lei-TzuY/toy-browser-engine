// ============================================================
// image_ico_bitfields_final.rs — masked Windows ICO DIB facade
// ============================================================

use std::collections::HashMap;
use std::rc::Rc;

use crate::net::{LoadError, ResourceLoader, Url};

pub use crate::image_prev15::{ImageError, ImageFormat, RasterImage};

/// Decode image bytes into straight RGBA8, adding BI_BITFIELDS and
/// BI_ALPHABITFIELDS ICO entries on top of the PNG/BI_RGB ICO stack.
pub fn decode(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    let Some(entry) = selected_masked_ico_entry(bytes)? else {
        return crate::image_prev15::decode(bytes);
    };
    decode_masked_ico_entry(bytes, entry)
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    width: u32,
    height: u32,
    bit_depth: u16,
    size: usize,
    offset: usize,
    compression: u32,
}

fn u16_le(bytes: &[u8], offset: usize, what: &str) -> Result<u16, ImageError> {
    let raw = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| ImageError::Decode(format!("truncated ICO {what}")))?;
    Ok(u16::from_le_bytes(raw.try_into().unwrap()))
}

fn u32_le(bytes: &[u8], offset: usize, what: &str) -> Result<u32, ImageError> {
    let raw = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| ImageError::Decode(format!("truncated ICO {what}")))?;
    Ok(u32::from_le_bytes(raw.try_into().unwrap()))
}

fn i32_le(bytes: &[u8], offset: usize, what: &str) -> Result<i32, ImageError> {
    let raw = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| ImageError::Decode(format!("truncated ICO {what}")))?;
    Ok(i32::from_le_bytes(raw.try_into().unwrap()))
}

fn selected_masked_ico_entry(bytes: &[u8]) -> Result<Option<Entry>, ImageError> {
    if bytes.len() < 6 || bytes[0..4] != [0, 0, 1, 0] {
        return Ok(None);
    }
    if u16_le(bytes, 0, "reserved field")? != 0 || u16_le(bytes, 2, "type")? != 1 {
        return Ok(None);
    }
    let count = u16_le(bytes, 4, "image count")? as usize;
    if count == 0 {
        return Ok(None);
    }
    let directory_end = 6usize
        .checked_add(
            count
                .checked_mul(16)
                .ok_or_else(|| ImageError::Decode("ICO directory size overflow".into()))?,
        )
        .ok_or_else(|| ImageError::Decode("ICO directory size overflow".into()))?;
    if directory_end > bytes.len() {
        return Ok(None);
    }

    let mut ranked = Vec::with_capacity(count);
    for index in 0..count {
        let base = 6 + index * 16;
        let width = if bytes[base] == 0 { 256 } else { bytes[base] as u32 };
        let height = if bytes[base + 1] == 0 { 256 } else { bytes[base + 1] as u32 };
        if bytes[base + 3] != 0 {
            return Err(ImageError::Decode(format!(
                "ICO entry {index} reserved byte must be zero"
            )));
        }
        let bit_depth = u16_le(bytes, base + 6, "entry bit depth")?;
        let size = usize::try_from(u32_le(bytes, base + 8, "entry byte size")?)
            .map_err(|_| ImageError::Decode("ICO entry size does not fit this platform".into()))?;
        let offset = usize::try_from(u32_le(bytes, base + 12, "entry image offset")?)
            .map_err(|_| ImageError::Decode("ICO entry offset does not fit this platform".into()))?;
        let end = offset
            .checked_add(size)
            .ok_or_else(|| ImageError::Decode("ICO entry payload range overflow".into()))?;
        if size == 0 || offset < directory_end || end > bytes.len() {
            return Ok(None);
        }
        ranked.push((width.saturating_mul(height), bit_depth, index, width, height, size, offset));
    }
    ranked.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    let (_, bit_depth, _, width, height, size, offset) = ranked[0];
    let payload = &bytes[offset..offset + size];
    if payload.starts_with(b"\x89PNG\r\n\x1a\n") || payload.len() < 20 {
        return Ok(None);
    }
    let compression = u32_le(payload, 16, "DIB compression")?;
    if !matches!(compression, 3 | 6) {
        return Ok(None);
    }
    Ok(Some(Entry {
        width,
        height,
        bit_depth,
        size,
        offset,
        compression,
    }))
}

fn stride(width: u32, depth: u16, what: &str) -> Result<usize, ImageError> {
    let bits = usize::try_from(width)
        .map_err(|_| ImageError::Decode(format!("ICO {what} width does not fit this platform")))?
        .checked_mul(depth as usize)
        .ok_or_else(|| ImageError::Decode(format!("ICO {what} row size overflow")))?;
    bits.checked_add(31)
        .map(|n| (n / 32) * 4)
        .ok_or_else(|| ImageError::Decode(format!("ICO {what} row size overflow")))
}

fn decode_masked_ico_entry(bytes: &[u8], entry: Entry) -> Result<RasterImage, ImageError> {
    let payload = &bytes[entry.offset..entry.offset + entry.size];
    if payload.len() < 40 {
        return Err(ImageError::Decode("truncated ICO bitfield DIB header".into()));
    }
    let header_size = usize::try_from(u32_le(payload, 0, "DIB header size")?)
        .map_err(|_| ImageError::Decode("ICO DIB header size does not fit this platform".into()))?;
    if header_size < 40 || header_size > payload.len() {
        return Err(ImageError::Decode(format!(
            "unsupported or truncated ICO DIB header size {header_size}"
        )));
    }
    let width_signed = i32_le(payload, 4, "DIB width")?;
    let stored_height = i32_le(payload, 8, "DIB height")?;
    if width_signed <= 0 || stored_height <= 0 || stored_height % 2 != 0 {
        return Err(ImageError::Decode(
            "ICO bitfield DIB requires positive width and doubled positive height".into(),
        ));
    }
    let width = width_signed as u32;
    let height = (stored_height / 2) as u32;
    if width != entry.width || height != entry.height {
        return Err(ImageError::Decode(format!(
            "ICO directory dimensions {}x{} do not match bitfield DIB {}x{}",
            entry.width, entry.height, width, height
        )));
    }
    if u16_le(payload, 12, "DIB planes")? != 1 {
        return Err(ImageError::Decode("ICO bitfield DIB requires one plane".into()));
    }
    let depth = u16_le(payload, 14, "DIB bit depth")?;
    if !matches!(depth, 16 | 32) {
        return Err(ImageError::Decode(
            "ICO bitfields support only 16- or 32-bit pixels".into(),
        ));
    }
    if entry.bit_depth != 0 && entry.bit_depth != depth {
        return Err(ImageError::Decode(format!(
            "ICO directory bit depth {} does not match bitfield DIB {depth}",
            entry.bit_depth
        )));
    }
    if entry.compression == 6 && depth != 32 {
        return Err(ImageError::Decode(
            "ICO BI_ALPHABITFIELDS requires 32-bit pixels".into(),
        ));
    }

    let mask_count = if entry.compression == 6 { 4usize } else { 3usize };
    let embedded_mask_end = 40 + mask_count * 4;
    let (masks, xor_start) = if header_size >= embedded_mask_end {
        (&payload[40..embedded_mask_end], header_size)
    } else if header_size == 40 {
        let end = 40usize
            .checked_add(mask_count * 4)
            .ok_or_else(|| ImageError::Decode("ICO bitfield mask range overflow".into()))?;
        let masks = payload
            .get(40..end)
            .ok_or_else(|| ImageError::Decode("truncated ICO bitfield masks".into()))?;
        (masks, end)
    } else {
        return Err(ImageError::Decode(format!(
            "ICO DIB header size {header_size} does not contain the required bitfield masks"
        )));
    };

    let xor_stride = stride(width, depth, "bitfield XOR")?;
    let height_usize = height as usize;
    let xor_len = xor_stride
        .checked_mul(height_usize)
        .ok_or_else(|| ImageError::Decode("ICO bitfield XOR size overflow".into()))?;
    let xor_end = xor_start
        .checked_add(xor_len)
        .ok_or_else(|| ImageError::Decode("ICO bitfield XOR range overflow".into()))?;
    if xor_end > payload.len() {
        return Err(ImageError::Decode("truncated ICO bitfield XOR bitmap".into()));
    }

    let and_stride = stride(width, 1, "bitfield AND mask")?;
    let and_len = and_stride
        .checked_mul(height_usize)
        .ok_or_else(|| ImageError::Decode("ICO bitfield AND mask size overflow".into()))?;
    let and_end = xor_end
        .checked_add(and_len)
        .ok_or_else(|| ImageError::Decode("ICO bitfield AND mask range overflow".into()))?;
    let mask_present = and_end <= payload.len();
    if payload.len() > xor_end && !mask_present {
        return Err(ImageError::Decode("truncated ICO bitfield AND mask".into()));
    }
    if entry.compression == 3 && !mask_present {
        return Err(ImageError::Decode("missing ICO bitfield AND mask".into()));
    }

    // Normalize the icon's XOR DIB into a standalone 40-byte-header BMP so the
    // existing, hardened BI_BITFIELDS/BI_ALPHABITFIELDS decoder owns mask
    // validation and channel scaling. The ICO-specific AND mask stays outside.
    let bmp_pixel_offset = 14usize + 40 + mask_count * 4;
    let bmp_len = bmp_pixel_offset
        .checked_add(xor_len)
        .ok_or_else(|| ImageError::Decode("normalized ICO BMP size overflow".into()))?;
    let bmp_len_u32 = u32::try_from(bmp_len)
        .map_err(|_| ImageError::Decode("normalized ICO BMP is too large".into()))?;
    let pixel_offset_u32 = u32::try_from(bmp_pixel_offset)
        .map_err(|_| ImageError::Decode("normalized ICO BMP pixel offset is too large".into()))?;
    let xor_len_u32 = u32::try_from(xor_len)
        .map_err(|_| ImageError::Decode("normalized ICO XOR bitmap is too large".into()))?;
    let height_i32 = i32::try_from(height)
        .map_err(|_| ImageError::Decode("ICO DIB height exceeds BMP limits".into()))?;
    let mut bmp = vec![0u8; bmp_len];
    bmp[0..2].copy_from_slice(b"BM");
    bmp[2..6].copy_from_slice(&bmp_len_u32.to_le_bytes());
    bmp[10..14].copy_from_slice(&pixel_offset_u32.to_le_bytes());
    bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
    bmp[18..22].copy_from_slice(&width_signed.to_le_bytes());
    bmp[22..26].copy_from_slice(&height_i32.to_le_bytes());
    bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
    bmp[28..30].copy_from_slice(&depth.to_le_bytes());
    bmp[30..34].copy_from_slice(&entry.compression.to_le_bytes());
    bmp[34..38].copy_from_slice(&xor_len_u32.to_le_bytes());
    bmp[54..54 + masks.len()].copy_from_slice(masks);
    bmp[bmp_pixel_offset..].copy_from_slice(&payload[xor_start..xor_end]);

    let mut image = crate::image_prev15::decode(&bmp)?;
    if image.width != width || image.height != height {
        return Err(ImageError::Decode("normalized ICO bitfield BMP changed dimensions".into()));
    }

    // BI_ALPHABITFIELDS carries an explicit alpha channel, which is
    // authoritative. Plain BI_BITFIELDS has no alpha channel, so ICO's AND
    // mask supplies binary transparency.
    if entry.compression == 3 {
        let mask = &payload[xor_end..and_end];
        for y in 0..height_usize {
            let source_y = height_usize - 1 - y;
            let row = &mask[source_y * and_stride..(source_y + 1) * and_stride];
            for x in 0..width as usize {
                if (row[x / 8] >> (7 - (x % 8))) & 1 != 0 {
                    image.pixels[(y * width as usize + x) * 4 + 3] = 0;
                }
            }
        }
    }
    Ok(image)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ico(width: u8, height: u8, depth: u16, payload: Vec<u8>) -> Vec<u8> {
        let mut out = vec![0u8; 22];
        out[2..4].copy_from_slice(&1u16.to_le_bytes());
        out[4..6].copy_from_slice(&1u16.to_le_bytes());
        out[6] = width;
        out[7] = height;
        out[10..12].copy_from_slice(&1u16.to_le_bytes());
        out[12..14].copy_from_slice(&depth.to_le_bytes());
        out[14..18].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        out[18..22].copy_from_slice(&22u32.to_le_bytes());
        out.extend_from_slice(&payload);
        out
    }

    fn bitfield_dib(
        depth: u16,
        compression: u32,
        masks: &[u32],
        pixel: &[u8],
        and_mask: &[u8],
    ) -> Vec<u8> {
        let mut out = vec![0u8; 40 + masks.len() * 4];
        out[0..4].copy_from_slice(&40u32.to_le_bytes());
        out[4..8].copy_from_slice(&1i32.to_le_bytes());
        out[8..12].copy_from_slice(&2i32.to_le_bytes());
        out[12..14].copy_from_slice(&1u16.to_le_bytes());
        out[14..16].copy_from_slice(&depth.to_le_bytes());
        out[16..20].copy_from_slice(&compression.to_le_bytes());
        for (index, mask) in masks.iter().copied().enumerate() {
            let base = 40 + index * 4;
            out[base..base + 4].copy_from_slice(&mask.to_le_bytes());
        }
        out.extend_from_slice(pixel);
        out.extend_from_slice(and_mask);
        out
    }

    #[test]
    fn decodes_rgb565_and_applies_and_mask() {
        let dib = bitfield_dib(
            16,
            3,
            &[0xf800, 0x07e0, 0x001f],
            &[0x00, 0xf8, 0, 0],
            &[0x80, 0, 0, 0],
        );
        let image = decode(&ico(1, 1, 16, dib)).unwrap();
        assert_eq!(image.pixel(0, 0), [255, 0, 0, 0]);
    }

    #[test]
    fn explicit_alpha_mask_is_authoritative() {
        let dib = bitfield_dib(
            32,
            6,
            &[0x00ff0000, 0x0000ff00, 0x000000ff, 0xff000000],
            &[0x33, 0x22, 0x11, 0x80],
            &[0x80, 0, 0, 0],
        );
        let image = decode(&ico(1, 1, 32, dib)).unwrap();
        assert_eq!(image.pixel(0, 0), [0x11, 0x22, 0x33, 0x80]);
    }

    #[test]
    fn rejects_missing_plain_bitfield_and_mask() {
        let dib = bitfield_dib(16, 3, &[0xf800, 0x07e0, 0x001f], &[0, 0, 0, 0], &[]);
        assert!(decode(&ico(1, 1, 16, dib)).is_err());
    }
}
