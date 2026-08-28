// ============================================================
// image_ico_os2_core_final.rs — OS/2 BITMAPCOREHEADER ICO facade
// ============================================================

use std::collections::HashMap;
use std::rc::Rc;

use crate::net::{LoadError, ResourceLoader, Url};

pub use crate::image_prev17::{ImageError, ImageFormat, RasterImage};

/// Decode straight RGBA8 images, adding OS/2 1.x BITMAPCOREHEADER-backed
/// ICO entries on top of the existing image/ICO decoder stack.
pub fn decode(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    if looks_like_ico(bytes) {
        decode_ico(bytes)
    } else {
        crate::image_prev17::decode(bytes)
    }
}

fn looks_like_ico(bytes: &[u8]) -> bool {
    bytes.len() >= 6 && bytes[0..4] == [0, 0, 1, 0]
}

fn read_u16(bytes: &[u8], offset: usize, field: &str) -> Result<u16, ImageError> {
    let end = offset
        .checked_add(2)
        .ok_or_else(|| ImageError::Decode(format!("ICO {field} offset overflow")))?;
    let raw: [u8; 2] = bytes
        .get(offset..end)
        .ok_or_else(|| ImageError::Decode(format!("truncated ICO {field}")))?
        .try_into()
        .expect("slice length checked");
    Ok(u16::from_le_bytes(raw))
}

fn read_u32(bytes: &[u8], offset: usize, field: &str) -> Result<u32, ImageError> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| ImageError::Decode(format!("ICO {field} offset overflow")))?;
    let raw: [u8; 4] = bytes
        .get(offset..end)
        .ok_or_else(|| ImageError::Decode(format!("truncated ICO {field}")))?
        .try_into()
        .expect("slice length checked");
    Ok(u32::from_le_bytes(raw))
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
    let directory_bytes = count
        .checked_mul(16)
        .ok_or_else(|| ImageError::Decode("ICO directory size overflow".into()))?;
    let directory_end = 6usize
        .checked_add(directory_bytes)
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
    if payload.len() < 4 || read_u32(payload, 0, "DIB header size")? != 12 {
        return crate::image_prev17::decode(bytes);
    }
    decode_core_dib(payload, selected)
}

fn dib_stride(width: usize, depth: usize, what: &str) -> Result<usize, ImageError> {
    let bits = width
        .checked_mul(depth)
        .ok_or_else(|| ImageError::Decode(format!("ICO {what} row bit count overflow")))?;
    let bytes = bits
        .checked_add(7)
        .map(|n| n / 8)
        .ok_or_else(|| ImageError::Decode(format!("ICO {what} row payload overflow")))?;
    bytes
        .checked_add(3)
        .map(|n| n & !3)
        .ok_or_else(|| ImageError::Decode(format!("ICO {what} row padding overflow")))
}

fn palette_entry(palette: &[[u8; 4]], index: usize) -> Result<[u8; 4], ImageError> {
    palette.get(index).copied().ok_or_else(|| {
        ImageError::Decode(format!(
            "ICO OS/2 DIB palette index {index} exceeds declared palette"
        ))
    })
}

fn decode_core_dib(payload: &[u8], entry: Entry) -> Result<RasterImage, ImageError> {
    const CORE_HEADER: usize = 12;

    if payload.len() < CORE_HEADER {
        return Err(ImageError::Decode("truncated ICO OS/2 core header".into()));
    }
    if read_u32(payload, 0, "DIB header size")? != 12 {
        return Err(ImageError::Decode("ICO payload is not an OS/2 BITMAPCOREHEADER".into()));
    }

    let width = read_u16(payload, 4, "OS/2 DIB width")? as usize;
    let stored_height = read_u16(payload, 6, "OS/2 DIB stored height")? as usize;
    if width == 0 || stored_height == 0 || stored_height % 2 != 0 {
        return Err(ImageError::Decode(
            "ICO OS/2 DIB dimensions must be non-zero and stored height must contain XOR+AND rows"
                .into(),
        ));
    }
    let height = stored_height / 2;
    if width as u32 != entry.width || height as u32 != entry.height {
        return Err(ImageError::Decode(format!(
            "ICO directory dimensions {}x{} do not match OS/2 DIB payload {}x{}",
            entry.width, entry.height, width, height
        )));
    }
    if read_u16(payload, 8, "OS/2 DIB planes")? != 1 {
        return Err(ImageError::Decode("ICO OS/2 DIB must have one color plane".into()));
    }
    let depth = read_u16(payload, 10, "OS/2 DIB bit depth")?;
    if !matches!(depth, 1 | 4 | 8 | 24) {
        return Err(ImageError::Decode(format!(
            "unsupported ICO OS/2 DIB bit depth {depth}"
        )));
    }
    if entry.bit_depth != 0 && entry.bit_depth != depth {
        return Err(ImageError::Decode(format!(
            "ICO directory bit depth {} does not match OS/2 DIB payload {depth}",
            entry.bit_depth
        )));
    }

    let palette_entries = match depth {
        1 => 2usize,
        4 => 16usize,
        8 => 256usize,
        24 => 0usize,
        _ => unreachable!(),
    };
    let palette_bytes = palette_entries
        .checked_mul(3)
        .ok_or_else(|| ImageError::Decode("ICO OS/2 DIB palette size overflow".into()))?;
    let xor_start = CORE_HEADER
        .checked_add(palette_bytes)
        .ok_or_else(|| ImageError::Decode("ICO OS/2 DIB palette offset overflow".into()))?;
    if xor_start > payload.len() {
        return Err(ImageError::Decode("truncated ICO OS/2 RGBTRIPLE palette".into()));
    }

    let mut palette = Vec::with_capacity(palette_entries);
    for index in 0..palette_entries {
        let base = CORE_HEADER + index * 3;
        let triple = payload
            .get(base..base + 3)
            .ok_or_else(|| ImageError::Decode("truncated ICO OS/2 RGBTRIPLE palette".into()))?;
        palette.push([triple[2], triple[1], triple[0], 255]);
    }

    let xor_stride = dib_stride(width, depth as usize, "OS/2 XOR")?;
    let xor_bytes = xor_stride
        .checked_mul(height)
        .ok_or_else(|| ImageError::Decode("ICO OS/2 XOR bitmap size overflow".into()))?;
    let xor_end = xor_start
        .checked_add(xor_bytes)
        .ok_or_else(|| ImageError::Decode("ICO OS/2 XOR bitmap range overflow".into()))?;
    if xor_end > payload.len() {
        return Err(ImageError::Decode("truncated ICO OS/2 XOR bitmap".into()));
    }

    let and_stride = dib_stride(width, 1, "OS/2 AND mask")?;
    let and_bytes = and_stride
        .checked_mul(height)
        .ok_or_else(|| ImageError::Decode("ICO OS/2 AND-mask size overflow".into()))?;
    let and_end = xor_end
        .checked_add(and_bytes)
        .ok_or_else(|| ImageError::Decode("ICO OS/2 AND-mask range overflow".into()))?;
    if and_end > payload.len() {
        return Err(ImageError::Decode("missing or truncated ICO OS/2 AND mask".into()));
    }

    let rgba_len = width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| ImageError::Decode("ICO OS/2 RGBA size overflow".into()))?;
    let mut pixels = vec![0u8; rgba_len];

    for file_y in 0..height {
        let image_y = height - 1 - file_y;
        let row_start = xor_start + file_y * xor_stride;
        let row = &payload[row_start..row_start + xor_stride];
        for x in 0..width {
            let rgba = match depth {
                1 => {
                    let byte = row[x / 8];
                    let index = ((byte >> (7 - (x % 8))) & 1) as usize;
                    palette_entry(&palette, index)?
                }
                4 => {
                    let byte = row[x / 2];
                    let index = if x % 2 == 0 { byte >> 4 } else { byte & 0x0f } as usize;
                    palette_entry(&palette, index)?
                }
                8 => palette_entry(&palette, row[x] as usize)?,
                24 => {
                    let src = x
                        .checked_mul(3)
                        .ok_or_else(|| ImageError::Decode("ICO OS/2 pixel offset overflow".into()))?;
                    [row[src + 2], row[src + 1], row[src], 255]
                }
                _ => unreachable!(),
            };
            let dst = (image_y * width + x) * 4;
            pixels[dst..dst + 4].copy_from_slice(&rgba);
        }
    }

    for file_y in 0..height {
        let image_y = height - 1 - file_y;
        let row_start = xor_end + file_y * and_stride;
        let row = &payload[row_start..row_start + and_stride];
        for x in 0..width {
            if ((row[x / 8] >> (7 - (x % 8))) & 1) != 0 {
                pixels[(image_y * width + x) * 4 + 3] = 0;
            }
        }
    }

    Ok(RasterImage::new(width as u32, height as u32, pixels))
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

    fn palette(size: usize) -> Vec<[u8; 3]> {
        let mut entries = vec![[0, 0, 0]; size];
        if size > 1 {
            entries[1] = [255, 0, 0];
        }
        if size > 2 {
            entries[2] = [0, 255, 0];
        }
        entries
    }

    fn core_ico(
        width: u16,
        height: u16,
        depth: u16,
        palette: &[[u8; 3]],
        xor: &[u8],
        and_mask: &[u8],
    ) -> Vec<u8> {
        let mut dib = vec![0u8; 12 + palette.len() * 3];
        dib[0..4].copy_from_slice(&12u32.to_le_bytes());
        dib[4..6].copy_from_slice(&width.to_le_bytes());
        dib[6..8].copy_from_slice(&height.saturating_mul(2).to_le_bytes());
        dib[8..10].copy_from_slice(&1u16.to_le_bytes());
        dib[10..12].copy_from_slice(&depth.to_le_bytes());
        for (index, rgb) in palette.iter().enumerate() {
            let base = 12 + index * 3;
            dib[base..base + 3].copy_from_slice(&[rgb[2], rgb[1], rgb[0]]);
        }
        dib.extend_from_slice(xor);
        dib.extend_from_slice(and_mask);

        let mut out = vec![0u8; 22];
        out[2..4].copy_from_slice(&1u16.to_le_bytes());
        out[4..6].copy_from_slice(&1u16.to_le_bytes());
        out[6] = if width == 256 { 0 } else { width as u8 };
        out[7] = if height == 256 { 0 } else { height as u8 };
        out[10..12].copy_from_slice(&1u16.to_le_bytes());
        out[12..14].copy_from_slice(&depth.to_le_bytes());
        out[14..18].copy_from_slice(&(dib.len() as u32).to_le_bytes());
        out[18..22].copy_from_slice(&22u32.to_le_bytes());
        out.extend_from_slice(&dib);
        out
    }

    #[test]
    fn decodes_24bit_bottom_up_and_and_mask() {
        let bytes = core_ico(
            1,
            2,
            24,
            &[],
            &[
                255, 0, 0, 0, // bottom blue
                0, 0, 255, 0, // top red
            ],
            &[
                0x80, 0, 0, 0, // bottom transparent
                0x00, 0, 0, 0, // top opaque
            ],
        );
        let image = decode(&bytes).expect("OS/2 core ICO");
        assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(image.pixel(0, 1), [0, 0, 255, 0]);
    }

    #[test]
    fn decodes_rgbtriple_indexed_core_icons() {
        let p1 = palette(2);
        let one = core_ico(2, 1, 1, &p1, &[0b0100_0000, 0, 0, 0], &[0, 0, 0, 0]);
        let image = decode(&one).expect("1-bit OS/2 ICO");
        assert_eq!(image.pixel(0, 0), [0, 0, 0, 255]);
        assert_eq!(image.pixel(1, 0), [255, 0, 0, 255]);

        let p4 = palette(16);
        let four = core_ico(2, 1, 4, &p4, &[0x12, 0, 0, 0], &[0, 0, 0, 0]);
        let image = decode(&four).expect("4-bit OS/2 ICO");
        assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(image.pixel(1, 0), [0, 255, 0, 255]);
    }

    #[test]
    fn rejects_dimension_depth_and_mask_errors() {
        let p = palette(2);
        let mut odd_height = core_ico(1, 1, 1, &p, &[0; 4], &[0; 4]);
        let payload = 22;
        odd_height[payload + 6..payload + 8].copy_from_slice(&3u16.to_le_bytes());
        assert!(decode(&odd_height).is_err());

        let bad_depth = core_ico(1, 1, 2, &[], &[0; 4], &[0; 4]);
        assert!(decode(&bad_depth).is_err());

        let mut truncated = core_ico(1, 1, 1, &p, &[0; 4], &[0; 4]);
        truncated.truncate(truncated.len() - 1);
        let new_size = (truncated.len() - 22) as u32;
        truncated[14..18].copy_from_slice(&new_size.to_le_bytes());
        assert!(decode(&truncated).is_err());
    }
}
