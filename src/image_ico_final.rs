// ============================================================
// image_ico_final.rs — Windows ICO favicon container facade
// ============================================================

use std::collections::HashMap;
use std::rc::Rc;

use crate::net::{LoadError, ResourceLoader, Url};

pub use crate::image_prev14::{ImageError, ImageFormat, RasterImage};

/// Decode image bytes into straight RGBA8, adding Windows ICO container support.
///
/// ICO entries can contain either PNG payloads or classic packed DIBs. The
/// container parser validates the directory, deterministically chooses the
/// largest advertised image (breaking ties by bit depth and then directory
/// order), bounds-checks the selected payload, and then dispatches to the
/// appropriate decoder.
pub fn decode(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    if looks_like_ico(bytes) {
        decode_ico(bytes)
    } else {
        crate::image_prev14::decode(bytes)
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
        let width = if bytes[base] == 0 {
            256
        } else {
            bytes[base] as u32
        };
        let height = if bytes[base + 1] == 0 {
            256
        } else {
            bytes[base + 1] as u32
        };
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
            return Err(ImageError::Decode(format!(
                "ICO entry {index} has zero byte size"
            )));
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
        let image = crate::image_prev14::decode(payload)?;
        if image.width != selected.width || image.height != selected.height {
            return Err(ImageError::Decode(format!(
                "ICO directory dimensions {}x{} do not match PNG payload {}x{}",
                selected.width, selected.height, image.width, image.height
            )));
        }
        return Ok(image);
    }

    decode_dib_icon(payload, selected)
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

fn scale_5_to_8(value: u16) -> u8 {
    ((value as u32 * 255 + 15) / 31) as u8
}

fn decode_dib_icon(payload: &[u8], entry: Entry) -> Result<RasterImage, ImageError> {
    if payload.len() < 40 {
        return Err(ImageError::Decode("truncated ICO DIB header".into()));
    }

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
            "ICO DIB dimensions must be positive and the stored height must contain XOR+AND rows"
                .into(),
        ));
    }
    let width = u32::try_from(dib_width)
        .map_err(|_| ImageError::Decode("ICO DIB width is out of range".into()))?;
    let height = u32::try_from(stored_height / 2)
        .map_err(|_| ImageError::Decode("ICO DIB height is out of range".into()))?;
    if width != entry.width || height != entry.height {
        return Err(ImageError::Decode(format!(
            "ICO directory dimensions {}x{} do not match DIB payload {}x{}",
            entry.width, entry.height, width, height
        )));
    }

    let planes = read_u16(payload, 12, "DIB planes")?;
    if planes != 1 {
        return Err(ImageError::Decode(format!(
            "ICO DIB must have one color plane, found {planes}"
        )));
    }
    let bit_depth = read_u16(payload, 14, "DIB bit depth")?;
    if !matches!(bit_depth, 1 | 4 | 8 | 16 | 24 | 32) {
        return Err(ImageError::Decode(format!(
            "unsupported ICO DIB bit depth {bit_depth}"
        )));
    }
    if entry.bit_depth != 0 && entry.bit_depth != bit_depth {
        return Err(ImageError::Decode(format!(
            "ICO directory bit depth {} does not match DIB payload {bit_depth}",
            entry.bit_depth
        )));
    }
    let compression = read_u32(payload, 16, "DIB compression")?;
    if compression != 0 {
        return Err(ImageError::Decode(format!(
            "unsupported ICO DIB compression {compression}; only BI_RGB is supported"
        )));
    }

    let palette_count = if bit_depth <= 8 {
        let declared = usize::try_from(read_u32(payload, 32, "DIB palette size")?)
            .map_err(|_| ImageError::Decode("ICO DIB palette size does not fit this platform".into()))?;
        let maximum = 1usize << bit_depth;
        let count = if declared == 0 { maximum } else { declared };
        if count == 0 || count > maximum {
            return Err(ImageError::Decode(format!(
                "ICO DIB palette contains {count} entries for {bit_depth}-bit pixels"
            )));
        }
        count
    } else {
        0
    };

    let palette_bytes = palette_count
        .checked_mul(4)
        .ok_or_else(|| ImageError::Decode("ICO DIB palette size overflow".into()))?;
    let xor_start = header_size
        .checked_add(palette_bytes)
        .ok_or_else(|| ImageError::Decode("ICO DIB pixel offset overflow".into()))?;
    if xor_start > payload.len() {
        return Err(ImageError::Decode("truncated ICO DIB palette".into()));
    }

    let xor_stride = dib_stride(width, bit_depth, "XOR")?;
    let height_usize = usize::try_from(height)
        .map_err(|_| ImageError::Decode("ICO DIB height does not fit this platform".into()))?;
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
    let mask_present = and_end <= payload.len();
    if payload.len() > xor_end && !mask_present {
        return Err(ImageError::Decode("truncated ICO DIB AND mask".into()));
    }
    if bit_depth != 32 && !mask_present {
        return Err(ImageError::Decode("missing ICO DIB AND mask".into()));
    }

    let mut palette = Vec::with_capacity(palette_count);
    for index in 0..palette_count {
        let base = header_size + index * 4;
        let color = payload
            .get(base..base + 4)
            .ok_or_else(|| ImageError::Decode("truncated ICO DIB palette".into()))?;
        palette.push([color[2], color[1], color[0], 255]);
    }

    let pixel_count = usize::try_from(width)
        .map_err(|_| ImageError::Decode("ICO DIB width does not fit this platform".into()))?
        .checked_mul(height_usize)
        .ok_or_else(|| ImageError::Decode("ICO DIB pixel count overflow".into()))?;
    let mut pixels = vec![0u8; pixel_count
        .checked_mul(4)
        .ok_or_else(|| ImageError::Decode("ICO DIB RGBA size overflow".into()))?];
    let mut has_nonzero_alpha = false;

    for y in 0..height_usize {
        let source_y = height_usize - 1 - y;
        let row_start = xor_start + source_y * xor_stride;
        let row = &payload[row_start..row_start + xor_stride];
        for x in 0..width as usize {
            let rgba = match bit_depth {
                1 => {
                    let index = ((row[x / 8] >> (7 - (x % 8))) & 1) as usize;
                    *palette.get(index).ok_or_else(|| {
                        ImageError::Decode(format!("ICO DIB palette index {index} is out of bounds"))
                    })?
                }
                4 => {
                    let packed = row[x / 2];
                    let index = if x % 2 == 0 { packed >> 4 } else { packed & 0x0f } as usize;
                    *palette.get(index).ok_or_else(|| {
                        ImageError::Decode(format!("ICO DIB palette index {index} is out of bounds"))
                    })?
                }
                8 => {
                    let index = row[x] as usize;
                    *palette.get(index).ok_or_else(|| {
                        ImageError::Decode(format!("ICO DIB palette index {index} is out of bounds"))
                    })?
                }
                16 => {
                    let base = x * 2;
                    let value = u16::from_le_bytes([row[base], row[base + 1]]);
                    [
                        scale_5_to_8((value >> 10) & 0x1f),
                        scale_5_to_8((value >> 5) & 0x1f),
                        scale_5_to_8(value & 0x1f),
                        255,
                    ]
                }
                24 => {
                    let base = x * 3;
                    [row[base + 2], row[base + 1], row[base], 255]
                }
                32 => {
                    let base = x * 4;
                    let alpha = row[base + 3];
                    has_nonzero_alpha |= alpha != 0;
                    [row[base + 2], row[base + 1], row[base], alpha]
                }
                _ => unreachable!(),
            };
            let dst = (y * width as usize + x) * 4;
            pixels[dst..dst + 4].copy_from_slice(&rgba);
        }
    }

    // 32-bit Vista-style icons carry real per-pixel alpha in the XOR bitmap.
    // Older 32-bit icons often leave every alpha byte at zero and rely on the
    // classic one-bit AND mask instead; in that legacy case treat unmasked
    // pixels as opaque before applying the mask. Lower bit depths always use
    // the AND mask for binary transparency.
    let apply_mask = mask_present && (bit_depth != 32 || !has_nonzero_alpha);
    if bit_depth == 32 && mask_present && !has_nonzero_alpha {
        for pixel in pixels.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
    }
    if apply_mask {
        for y in 0..height_usize {
            let source_y = height_usize - 1 - y;
            let row_start = xor_end + source_y * and_stride;
            let row = &payload[row_start..row_start + and_stride];
            for x in 0..width as usize {
                let transparent = (row[x / 8] >> (7 - (x % 8))) & 1 != 0;
                if transparent {
                    pixels[(y * width as usize + x) * 4 + 3] = 0;
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

    fn png_rgba(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            let mut pixels = Vec::with_capacity((width * height * 4) as usize);
            for _ in 0..width * height {
                pixels.extend_from_slice(&rgba);
            }
            writer.write_image_data(&pixels).unwrap();
        }
        out
    }

    fn ico(entries: Vec<(u8, u8, u16, Vec<u8>)>) -> Vec<u8> {
        let count = entries.len();
        let mut out = vec![0u8; 6 + count * 16];
        out[2..4].copy_from_slice(&1u16.to_le_bytes());
        out[4..6].copy_from_slice(&(count as u16).to_le_bytes());
        let mut offset = out.len();
        for (i, (width, height, depth, payload)) in entries.iter().enumerate() {
            let base = 6 + i * 16;
            out[base] = *width;
            out[base + 1] = *height;
            out[base + 4..base + 6].copy_from_slice(&1u16.to_le_bytes());
            out[base + 6..base + 8].copy_from_slice(&depth.to_le_bytes());
            out[base + 8..base + 12].copy_from_slice(&(payload.len() as u32).to_le_bytes());
            out[base + 12..base + 16].copy_from_slice(&(offset as u32).to_le_bytes());
            offset += payload.len();
        }
        for (_, _, _, payload) in entries {
            out.extend_from_slice(&payload);
        }
        out
    }

    fn dib24(width: u32, height: u32, pixels_top_down: &[[u8; 3]], transparent: &[(u32, u32)]) -> Vec<u8> {
        assert_eq!(pixels_top_down.len(), (width * height) as usize);
        let xor_stride = ((width as usize * 24 + 31) / 32) * 4;
        let and_stride = ((width as usize + 31) / 32) * 4;
        let mut out = vec![0u8; 40 + xor_stride * height as usize + and_stride * height as usize];
        out[0..4].copy_from_slice(&40u32.to_le_bytes());
        out[4..8].copy_from_slice(&(width as i32).to_le_bytes());
        out[8..12].copy_from_slice(&((height as i32) * 2).to_le_bytes());
        out[12..14].copy_from_slice(&1u16.to_le_bytes());
        out[14..16].copy_from_slice(&24u16.to_le_bytes());
        for y in 0..height as usize {
            let src_y = height as usize - 1 - y;
            let row = 40 + y * xor_stride;
            for x in 0..width as usize {
                let rgb = pixels_top_down[src_y * width as usize + x];
                let base = row + x * 3;
                out[base..base + 3].copy_from_slice(&[rgb[2], rgb[1], rgb[0]]);
            }
        }
        let and_start = 40 + xor_stride * height as usize;
        for &(x, y) in transparent {
            let source_y = height as usize - 1 - y as usize;
            let byte = and_start + source_y * and_stride + x as usize / 8;
            out[byte] |= 1 << (7 - (x as usize % 8));
        }
        out
    }

    fn dib32(width: u32, height: u32, pixels_top_down: &[[u8; 4]], transparent: &[(u32, u32)]) -> Vec<u8> {
        assert_eq!(pixels_top_down.len(), (width * height) as usize);
        let xor_stride = width as usize * 4;
        let and_stride = ((width as usize + 31) / 32) * 4;
        let mut out = vec![0u8; 40 + xor_stride * height as usize + and_stride * height as usize];
        out[0..4].copy_from_slice(&40u32.to_le_bytes());
        out[4..8].copy_from_slice(&(width as i32).to_le_bytes());
        out[8..12].copy_from_slice(&((height as i32) * 2).to_le_bytes());
        out[12..14].copy_from_slice(&1u16.to_le_bytes());
        out[14..16].copy_from_slice(&32u16.to_le_bytes());
        for y in 0..height as usize {
            let src_y = height as usize - 1 - y;
            let row = 40 + y * xor_stride;
            for x in 0..width as usize {
                let rgba = pixels_top_down[src_y * width as usize + x];
                let base = row + x * 4;
                out[base..base + 4].copy_from_slice(&[rgba[2], rgba[1], rgba[0], rgba[3]]);
            }
        }
        let and_start = 40 + xor_stride * height as usize;
        for &(x, y) in transparent {
            let source_y = height as usize - 1 - y as usize;
            let byte = and_start + source_y * and_stride + x as usize / 8;
            out[byte] |= 1 << (7 - (x as usize % 8));
        }
        out
    }

    #[test]
    fn decodes_png_backed_icon() {
        let bytes = ico(vec![(1, 1, 32, png_rgba(1, 1, [11, 22, 33, 44]))]);
        let image = decode(&bytes).unwrap();
        assert_eq!((image.width, image.height), (1, 1));
        assert_eq!(image.pixel(0, 0), [11, 22, 33, 44]);
    }

    #[test]
    fn selects_largest_then_deepest_entry() {
        let small = png_rgba(1, 1, [255, 0, 0, 255]);
        let shallow = png_rgba(2, 2, [0, 255, 0, 255]);
        let deep = png_rgba(2, 2, [0, 0, 255, 255]);
        let bytes = ico(vec![(1, 1, 32, small), (2, 2, 8, shallow), (2, 2, 32, deep)]);
        let image = decode(&bytes).unwrap();
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(image.pixel(0, 0), [0, 0, 255, 255]);
    }

    #[test]
    fn decodes_24bit_dib_bottom_up_and_applies_and_mask() {
        let dib = dib24(
            2,
            2,
            &[
                [255, 0, 0],
                [0, 255, 0],
                [0, 0, 255],
                [255, 255, 0],
            ],
            &[(1, 0)],
        );
        let image = decode(&ico(vec![(2, 2, 24, dib)])).unwrap();
        assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(image.pixel(1, 0), [0, 255, 0, 0]);
        assert_eq!(image.pixel(0, 1), [0, 0, 255, 255]);
        assert_eq!(image.pixel(1, 1), [255, 255, 0, 255]);
    }

    #[test]
    fn meaningful_32bit_alpha_takes_precedence_over_and_mask() {
        let dib = dib32(1, 1, &[[10, 20, 30, 77]], &[(0, 0)]);
        let image = decode(&ico(vec![(1, 1, 32, dib)])).unwrap();
        assert_eq!(image.pixel(0, 0), [10, 20, 30, 77]);
    }

    #[test]
    fn zeroed_32bit_alpha_falls_back_to_and_mask() {
        let dib = dib32(2, 1, &[[10, 20, 30, 0], [40, 50, 60, 0]], &[(1, 0)]);
        let image = decode(&ico(vec![(2, 1, 32, dib)])).unwrap();
        assert_eq!(image.pixel(0, 0), [10, 20, 30, 255]);
        assert_eq!(image.pixel(1, 0), [40, 50, 60, 0]);
    }

    #[test]
    fn rejects_bad_directory_dimension_compression_and_mask() {
        let mut bytes = ico(vec![(1, 1, 32, png_rgba(1, 1, [1, 2, 3, 4]))]);
        bytes[18..22].copy_from_slice(&1u32.to_le_bytes());
        assert!(decode(&bytes).is_err());

        let bytes = ico(vec![(2, 2, 32, png_rgba(1, 1, [1, 2, 3, 4]))]);
        assert!(decode(&bytes).is_err());

        let mut compressed = dib24(1, 1, &[[1, 2, 3]], &[]);
        compressed[16..20].copy_from_slice(&1u32.to_le_bytes());
        assert!(decode(&ico(vec![(1, 1, 24, compressed)])).is_err());

        let mut truncated_mask = dib24(1, 1, &[[1, 2, 3]], &[]);
        truncated_mask.pop();
        assert!(decode(&ico(vec![(1, 1, 24, truncated_mask)])).is_err());
    }
}
