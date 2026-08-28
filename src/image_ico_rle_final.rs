// ============================================================
// image_ico_rle_final.rs — RLE-compressed ICO DIB facade
// ============================================================

use std::collections::HashMap;
use std::rc::Rc;

use crate::net::{LoadError, ResourceLoader, Url};

pub use crate::image_prev16::{ImageError, ImageFormat, RasterImage};

/// Decode straight RGBA8 images, adding BI_RLE8/BI_RLE4 support for classic
/// DIB-backed ICO entries on top of the existing image/ICO decoder stack.
pub fn decode(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    if looks_like_ico(bytes) {
        decode_ico(bytes)
    } else {
        crate::image_prev16::decode(bytes)
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

fn read_i32(bytes: &[u8], offset: usize, field: &str) -> Result<i32, ImageError> {
    Ok(read_u32(bytes, offset, field)? as i32)
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
    if payload.starts_with(b"\x89PNG\r\n\x1a\n") || payload.len() < 20 {
        return crate::image_prev16::decode(bytes);
    }

    let compression = read_u32(payload, 16, "DIB compression")?;
    if !matches!(compression, 1 | 2) {
        return crate::image_prev16::decode(bytes);
    }
    decode_rle_dib(payload, selected, compression)
}

fn palette_rgba(palette: &[u8], index: usize) -> Result<[u8; 4], ImageError> {
    let start = index
        .checked_mul(4)
        .ok_or_else(|| ImageError::Decode("ICO DIB palette index overflow".into()))?;
    let end = start
        .checked_add(4)
        .ok_or_else(|| ImageError::Decode("ICO DIB palette index overflow".into()))?;
    let entry = palette.get(start..end).ok_or_else(|| {
        ImageError::Decode(format!(
            "ICO DIB palette index {index} exceeds declared palette"
        ))
    })?;
    Ok([entry[2], entry[1], entry[0], 255])
}

fn and_stride(width: u32) -> Result<usize, ImageError> {
    let bits = usize::try_from(width)
        .map_err(|_| ImageError::Decode("ICO AND-mask width does not fit this platform".into()))?;
    let dwords = bits
        .checked_add(31)
        .ok_or_else(|| ImageError::Decode("ICO AND-mask row size overflow".into()))?
        / 32;
    dwords
        .checked_mul(4)
        .ok_or_else(|| ImageError::Decode("ICO AND-mask row size overflow".into()))
}

fn write_indexed_pixel(
    pixels: &mut [u8],
    width: usize,
    height: usize,
    x: usize,
    file_y: usize,
    rgba: [u8; 4],
) -> Result<(), ImageError> {
    if x >= width || file_y >= height {
        return Err(ImageError::Decode("ICO RLE pixel exceeds image bounds".into()));
    }
    let y = height - 1 - file_y;
    let pixel = y
        .checked_mul(width)
        .and_then(|row| row.checked_add(x))
        .ok_or_else(|| ImageError::Decode("ICO RLE pixel offset overflow".into()))?;
    let dst = pixel
        .checked_mul(4)
        .ok_or_else(|| ImageError::Decode("ICO RLE RGBA offset overflow".into()))?;
    pixels[dst..dst + 4].copy_from_slice(&rgba);
    Ok(())
}

fn decode_rle_stream(
    stream: &[u8],
    compression: u32,
    palette: &[u8],
    width: usize,
    height: usize,
    pixels: &mut [u8],
) -> Result<usize, ImageError> {
    let mut pos = 0usize;
    let mut x = 0usize;
    let mut file_y = 0usize;

    while pos < stream.len() {
        let count = *stream
            .get(pos)
            .ok_or_else(|| ImageError::Decode("truncated ICO RLE command".into()))?;
        let value = *stream
            .get(pos + 1)
            .ok_or_else(|| ImageError::Decode("truncated ICO RLE command".into()))?;
        pos += 2;

        if count != 0 {
            let run = count as usize;
            let end_x = x
                .checked_add(run)
                .ok_or_else(|| ImageError::Decode("ICO RLE encoded run overflow".into()))?;
            if file_y >= height || end_x > width {
                return Err(ImageError::Decode("ICO RLE encoded run exceeds row bounds".into()));
            }

            if compression == 1 {
                let rgba = palette_rgba(palette, value as usize)?;
                for offset in 0..run {
                    write_indexed_pixel(pixels, width, height, x + offset, file_y, rgba)?;
                }
            } else {
                let hi = (value >> 4) as usize;
                let lo = (value & 0x0f) as usize;
                for offset in 0..run {
                    let index = if offset % 2 == 0 { hi } else { lo };
                    let rgba = palette_rgba(palette, index)?;
                    write_indexed_pixel(pixels, width, height, x + offset, file_y, rgba)?;
                }
            }
            x = end_x;
            continue;
        }

        match value {
            0 => {
                if file_y >= height {
                    return Err(ImageError::Decode("ICO RLE EOL exceeds image height".into()));
                }
                x = 0;
                file_y += 1;
            }
            1 => return Ok(pos),
            2 => {
                let dx = *stream
                    .get(pos)
                    .ok_or_else(|| ImageError::Decode("truncated ICO RLE delta".into()))?
                    as usize;
                let dy = *stream
                    .get(pos + 1)
                    .ok_or_else(|| ImageError::Decode("truncated ICO RLE delta".into()))?
                    as usize;
                pos += 2;
                x = x
                    .checked_add(dx)
                    .ok_or_else(|| ImageError::Decode("ICO RLE delta x overflow".into()))?;
                file_y = file_y
                    .checked_add(dy)
                    .ok_or_else(|| ImageError::Decode("ICO RLE delta y overflow".into()))?;
                if x > width || file_y >= height {
                    return Err(ImageError::Decode("ICO RLE delta exceeds image bounds".into()));
                }
            }
            literal_count => {
                let n = literal_count as usize;
                let end_x = x
                    .checked_add(n)
                    .ok_or_else(|| ImageError::Decode("ICO RLE absolute run overflow".into()))?;
                if file_y >= height || end_x > width {
                    return Err(ImageError::Decode("ICO RLE absolute run exceeds row bounds".into()));
                }

                if compression == 1 {
                    let end = pos
                        .checked_add(n)
                        .ok_or_else(|| ImageError::Decode("ICO RLE8 absolute range overflow".into()))?;
                    let literal = stream
                        .get(pos..end)
                        .ok_or_else(|| ImageError::Decode("truncated ICO RLE8 absolute run".into()))?;
                    for (offset, index) in literal.iter().copied().enumerate() {
                        let rgba = palette_rgba(palette, index as usize)?;
                        write_indexed_pixel(pixels, width, height, x + offset, file_y, rgba)?;
                    }
                    pos = end;
                    if n % 2 == 1 {
                        pos = pos
                            .checked_add(1)
                            .ok_or_else(|| ImageError::Decode("ICO RLE8 padding overflow".into()))?;
                        if pos > stream.len() {
                            return Err(ImageError::Decode("truncated ICO RLE8 absolute padding".into()));
                        }
                    }
                } else {
                    let packed_len = n
                        .checked_add(1)
                        .ok_or_else(|| ImageError::Decode("ICO RLE4 absolute size overflow".into()))?
                        / 2;
                    let end = pos
                        .checked_add(packed_len)
                        .ok_or_else(|| ImageError::Decode("ICO RLE4 absolute range overflow".into()))?;
                    let packed = stream
                        .get(pos..end)
                        .ok_or_else(|| ImageError::Decode("truncated ICO RLE4 absolute run".into()))?;
                    for offset in 0..n {
                        let byte = packed[offset / 2];
                        let index = if offset % 2 == 0 {
                            byte >> 4
                        } else {
                            byte & 0x0f
                        };
                        let rgba = palette_rgba(palette, index as usize)?;
                        write_indexed_pixel(pixels, width, height, x + offset, file_y, rgba)?;
                    }
                    pos = end;
                    if packed_len % 2 == 1 {
                        pos = pos
                            .checked_add(1)
                            .ok_or_else(|| ImageError::Decode("ICO RLE4 padding overflow".into()))?;
                        if pos > stream.len() {
                            return Err(ImageError::Decode("truncated ICO RLE4 absolute padding".into()));
                        }
                    }
                }
                x = end_x;
            }
        }
    }

    Err(ImageError::Decode(
        "ICO RLE stream is missing end-of-bitmap marker".into(),
    ))
}

fn decode_rle_dib(payload: &[u8], entry: Entry, compression: u32) -> Result<RasterImage, ImageError> {
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
            "ICO RLE DIB dimensions must be positive and stored height must contain XOR+AND rows"
                .into(),
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
        return Err(ImageError::Decode("ICO RLE DIB must have one color plane".into()));
    }

    let bit_depth = read_u16(payload, 14, "DIB bit depth")?;
    let expected_depth = if compression == 1 { 8 } else { 4 };
    if bit_depth != expected_depth {
        return Err(ImageError::Decode(format!(
            "ICO BI_RLE{} requires {}-bit pixels, found {bit_depth}",
            if compression == 1 { 8 } else { 4 },
            expected_depth
        )));
    }
    if entry.bit_depth != 0 && entry.bit_depth != bit_depth {
        return Err(ImageError::Decode(format!(
            "ICO directory bit depth {} does not match DIB payload {bit_depth}",
            entry.bit_depth
        )));
    }

    let colors_used = read_u32(payload, 32, "DIB colors used")?;
    let max_palette = 1usize << bit_depth;
    let palette_len = if colors_used == 0 {
        max_palette
    } else {
        usize::try_from(colors_used)
            .map_err(|_| ImageError::Decode("ICO DIB palette size overflow".into()))?
    };
    if palette_len == 0 || palette_len > max_palette {
        return Err(ImageError::Decode(format!(
            "invalid ICO DIB palette size {palette_len} for {bit_depth}-bit RLE"
        )));
    }
    let palette_bytes = palette_len
        .checked_mul(4)
        .ok_or_else(|| ImageError::Decode("ICO DIB palette size overflow".into()))?;
    let xor_start = header_size
        .checked_add(palette_bytes)
        .ok_or_else(|| ImageError::Decode("ICO DIB palette range overflow".into()))?;
    if xor_start > payload.len() {
        return Err(ImageError::Decode("truncated ICO DIB color palette".into()));
    }
    let palette = &payload[header_size..xor_start];

    let width_usize = width as usize;
    let height_usize = height as usize;
    let rgba_len = width_usize
        .checked_mul(height_usize)
        .and_then(|count| count.checked_mul(4))
        .ok_or_else(|| ImageError::Decode("ICO DIB RGBA size overflow".into()))?;
    let background = palette_rgba(palette, 0)?;
    let mut pixels = vec![0u8; rgba_len];
    for chunk in pixels.chunks_exact_mut(4) {
        chunk.copy_from_slice(&background);
    }

    let size_image = usize::try_from(read_u32(payload, 20, "DIB image size")?)
        .map_err(|_| ImageError::Decode("ICO DIB image size does not fit this platform".into()))?;
    let stream_limit = if size_image == 0 {
        payload.len()
    } else {
        xor_start
            .checked_add(size_image)
            .ok_or_else(|| ImageError::Decode("ICO DIB RLE stream range overflow".into()))?
    };
    if stream_limit > payload.len() {
        return Err(ImageError::Decode("truncated ICO DIB RLE stream".into()));
    }

    let consumed = decode_rle_stream(
        &payload[xor_start..stream_limit],
        compression,
        palette,
        width_usize,
        height_usize,
        &mut pixels,
    )?;
    if size_image != 0 && consumed > size_image {
        return Err(ImageError::Decode(
            "ICO DIB RLE end marker exceeds declared image size".into(),
        ));
    }
    let and_start = if size_image == 0 {
        xor_start
            .checked_add(consumed)
            .ok_or_else(|| ImageError::Decode("ICO DIB RLE range overflow".into()))?
    } else {
        stream_limit
    };

    let mask_stride = and_stride(width)?;
    let mask_bytes = mask_stride
        .checked_mul(height_usize)
        .ok_or_else(|| ImageError::Decode("ICO DIB AND-mask size overflow".into()))?;
    let and_end = and_start
        .checked_add(mask_bytes)
        .ok_or_else(|| ImageError::Decode("ICO DIB AND-mask range overflow".into()))?;
    if and_end > payload.len() {
        return Err(ImageError::Decode(
            "missing or truncated ICO DIB AND mask after RLE stream".into(),
        ));
    }

    for y in 0..height_usize {
        let source_y = height_usize - 1 - y;
        let row_start = and_start + source_y * mask_stride;
        let row = &payload[row_start..row_start + mask_stride];
        for x in 0..width_usize {
            if ((row[x / 8] >> (7 - (x % 8))) & 1) != 0 {
                pixels[(y * width_usize + x) * 4 + 3] = 0;
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

    fn rle_ico(
        width: u8,
        height: u8,
        bit_depth: u16,
        compression: u32,
        palette: &[[u8; 4]],
        rle: &[u8],
        and_mask: &[u8],
        size_image: Option<u32>,
    ) -> Vec<u8> {
        let mut dib = vec![0u8; 40];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&(width as i32).to_le_bytes());
        dib[8..12].copy_from_slice(&((height as i32) * 2).to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&bit_depth.to_le_bytes());
        dib[16..20].copy_from_slice(&compression.to_le_bytes());
        dib[20..24].copy_from_slice(&size_image.unwrap_or(rle.len() as u32).to_le_bytes());
        dib[32..36].copy_from_slice(&(palette.len() as u32).to_le_bytes());
        for color in palette {
            dib.extend_from_slice(color);
        }
        dib.extend_from_slice(rle);
        dib.extend_from_slice(and_mask);

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
    fn decodes_rle8_rows_and_and_mask() {
        let palette = [
            [0, 0, 0, 0],
            [0, 0, 255, 0],
            [0, 255, 0, 0],
        ];
        let rle = [2, 1, 0, 0, 2, 2, 0, 0, 0, 1];
        let and_mask = [0x40, 0, 0, 0, 0, 0, 0, 0];
        let bytes = rle_ico(2, 2, 8, 1, &palette, &rle, &and_mask, None);
        let image = decode(&bytes).expect("RLE8 ICO");
        assert_eq!(image.pixel(0, 0), [0, 255, 0, 255]);
        assert_eq!(image.pixel(0, 1), [255, 0, 0, 255]);
        assert_eq!(image.pixel(1, 1), [255, 0, 0, 0]);
    }

    #[test]
    fn decodes_rle4_absolute_mode_with_zero_size_image() {
        let palette = [
            [0, 0, 0, 0],
            [0, 0, 255, 0],
            [0, 255, 0, 0],
            [255, 0, 0, 0],
        ];
        let rle = [0, 4, 0x12, 0x30, 0, 1];
        let and_mask = [0, 0, 0, 0];
        let bytes = rle_ico(4, 1, 4, 2, &palette, &rle, &and_mask, Some(0));
        let image = decode(&bytes).expect("RLE4 ICO");
        assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(image.pixel(1, 0), [0, 255, 0, 255]);
        assert_eq!(image.pixel(2, 0), [0, 0, 255, 255]);
        assert_eq!(image.pixel(3, 0), [0, 0, 0, 255]);
    }

    #[test]
    fn rejects_missing_eob_and_truncated_and_mask() {
        let palette = [[0, 0, 0, 0], [0, 0, 255, 0]];
        let missing_eob = rle_ico(1, 1, 8, 1, &palette, &[1, 1], &[0, 0, 0, 0], None);
        assert!(decode(&missing_eob).is_err());

        let truncated_mask = rle_ico(1, 1, 8, 1, &palette, &[1, 1, 0, 1], &[], None);
        assert!(decode(&truncated_mask).is_err());
    }
}
