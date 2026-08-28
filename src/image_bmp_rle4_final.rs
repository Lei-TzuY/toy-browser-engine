// ============================================================
//  image_bmp_rle4_final.rs — Windows BMP RLE4 image facade
// ============================================================

use std::collections::HashMap;
use std::rc::Rc;

use crate::net::{LoadError, ResourceLoader, Url};

pub use crate::image_prev8::{ImageError, ImageFormat, RasterImage};

/// Decode image bytes into straight RGBA8, adding Windows BI_RLE4 support on
/// top of the existing still-image/BMP decoder stack.
pub fn decode(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    if is_rle4_bmp(bytes) {
        decode_rle4_bmp(bytes)
    } else {
        crate::image_prev8::decode(bytes)
    }
}

fn is_rle4_bmp(bytes: &[u8]) -> bool {
    bytes.starts_with(b"BM")
        && bytes
            .get(30..34)
            .and_then(|raw| raw.try_into().ok())
            .map(u32::from_le_bytes)
            == Some(2)
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

fn palette_rgba(palette: &[u8], index: usize) -> Result<[u8; 4], ImageError> {
    let src = index
        .checked_mul(4)
        .ok_or_else(|| ImageError::Decode("BMP palette index overflow".into()))?;
    let entry = palette.get(src..src + 4).ok_or_else(|| {
        ImageError::Decode(format!(
            "BMP palette index {index} exceeds declared palette"
        ))
    })?;
    Ok([entry[2], entry[1], entry[0], 255])
}

fn write_rle4_pixel(
    pixels: &mut [u8],
    width: usize,
    height: usize,
    x: usize,
    file_y: usize,
    rgba: [u8; 4],
) -> Result<(), ImageError> {
    if x >= width || file_y >= height {
        return Err(ImageError::Decode("BI_RLE4 pixel exceeds image bounds".into()));
    }
    let image_y = height - 1 - file_y;
    let dst = (image_y * width + x)
        .checked_mul(4)
        .ok_or_else(|| ImageError::Decode("BI_RLE4 output offset overflow".into()))?;
    pixels[dst..dst + 4].copy_from_slice(&rgba);
    Ok(())
}

fn decode_rle4_bmp(bytes: &[u8]) -> Result<RasterImage, ImageError> {
    if bytes.len() < 54 {
        return Err(ImageError::Decode("truncated BMP header".into()));
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
    if dib_end > bytes.len() || pixel_offset < dib_end || pixel_offset > bytes.len() {
        return Err(ImageError::Decode("invalid BMP header/pixel offset".into()));
    }

    let width_signed = i32_le(bytes, 18, "width")?;
    let height_signed = i32_le(bytes, 22, "height")?;
    if width_signed <= 0 {
        return Err(ImageError::Decode("BMP width must be positive".into()));
    }
    if height_signed <= 0 {
        return Err(ImageError::Decode(
            "BI_RLE4 requires a positive bottom-up BMP height".into(),
        ));
    }
    if u16_le(bytes, 26, "planes")? != 1 {
        return Err(ImageError::Decode("BI_RLE4 requires one plane".into()));
    }
    if u16_le(bytes, 28, "bits per pixel")? != 4 {
        return Err(ImageError::Decode("BI_RLE4 requires 4-bit pixels".into()));
    }
    if u32_le(bytes, 30, "compression")? != 2 {
        return Err(ImageError::Decode("BMP is not BI_RLE4-compressed".into()));
    }

    let colors_used = u32_le(bytes, 46, "colors used")?;
    let palette_len = if colors_used == 0 {
        16usize
    } else {
        usize::try_from(colors_used)
            .map_err(|_| ImageError::Decode("BMP palette size overflow".into()))?
    };
    if palette_len == 0 || palette_len > 16 {
        return Err(ImageError::Decode(format!(
            "invalid BMP palette size {palette_len} for BI_RLE4"
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
    let palette = &bytes[dib_end..palette_end];

    let width = width_signed as u32;
    let height = height_signed as u32;
    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| ImageError::Decode("BMP dimensions overflow pixel count".into()))?;
    let capacity = pixel_count
        .checked_mul(4)
        .ok_or_else(|| ImageError::Decode("BMP dimensions overflow RGBA buffer".into()))?;
    let background = palette_rgba(palette, 0)?;
    let mut pixels = vec![0u8; capacity];
    for chunk in pixels.chunks_exact_mut(4) {
        chunk.copy_from_slice(&background);
    }

    let stream = &bytes[pixel_offset..];
    let mut pos = 0usize;
    let mut x = 0usize;
    let mut file_y = 0usize;
    let mut saw_eob = false;

    while pos < stream.len() {
        let count = *stream
            .get(pos)
            .ok_or_else(|| ImageError::Decode("truncated BI_RLE4 command".into()))?;
        let value = *stream
            .get(pos + 1)
            .ok_or_else(|| ImageError::Decode("truncated BI_RLE4 command".into()))?;
        pos += 2;

        if count != 0 {
            let run = count as usize;
            if file_y >= height as usize
                || x.checked_add(run).is_none_or(|end| end > width as usize)
            {
                return Err(ImageError::Decode("BI_RLE4 encoded run exceeds row bounds".into()));
            }
            let hi = (value >> 4) as usize;
            let lo = (value & 0x0f) as usize;
            for offset in 0..run {
                let index = if offset % 2 == 0 { hi } else { lo };
                let rgba = palette_rgba(palette, index)?;
                write_rle4_pixel(
                    &mut pixels,
                    width as usize,
                    height as usize,
                    x + offset,
                    file_y,
                    rgba,
                )?;
            }
            x += run;
            continue;
        }

        match value {
            0 => {
                if file_y >= height as usize {
                    return Err(ImageError::Decode("BI_RLE4 EOL exceeds image height".into()));
                }
                x = 0;
                file_y += 1;
            }
            1 => {
                saw_eob = true;
                break;
            }
            2 => {
                let dx = *stream
                    .get(pos)
                    .ok_or_else(|| ImageError::Decode("truncated BI_RLE4 delta".into()))?
                    as usize;
                let dy = *stream
                    .get(pos + 1)
                    .ok_or_else(|| ImageError::Decode("truncated BI_RLE4 delta".into()))?
                    as usize;
                pos += 2;
                x = x
                    .checked_add(dx)
                    .ok_or_else(|| ImageError::Decode("BI_RLE4 delta x overflow".into()))?;
                file_y = file_y
                    .checked_add(dy)
                    .ok_or_else(|| ImageError::Decode("BI_RLE4 delta y overflow".into()))?;
                if x > width as usize || file_y >= height as usize {
                    return Err(ImageError::Decode("BI_RLE4 delta exceeds image bounds".into()));
                }
            }
            literal_count => {
                let n = literal_count as usize;
                if file_y >= height as usize
                    || x.checked_add(n).is_none_or(|end| end > width as usize)
                {
                    return Err(ImageError::Decode("BI_RLE4 absolute run exceeds row bounds".into()));
                }
                let packed_len = n
                    .checked_add(1)
                    .ok_or_else(|| ImageError::Decode("BI_RLE4 absolute size overflow".into()))?
                    / 2;
                let end = pos
                    .checked_add(packed_len)
                    .ok_or_else(|| ImageError::Decode("BI_RLE4 absolute run overflow".into()))?;
                let packed = stream
                    .get(pos..end)
                    .ok_or_else(|| ImageError::Decode("truncated BI_RLE4 absolute run".into()))?;
                for offset in 0..n {
                    let byte = packed[offset / 2];
                    let index = if offset % 2 == 0 { byte >> 4 } else { byte & 0x0f };
                    let rgba = palette_rgba(palette, index as usize)?;
                    write_rle4_pixel(
                        &mut pixels,
                        width as usize,
                        height as usize,
                        x + offset,
                        file_y,
                        rgba,
                    )?;
                }
                pos = end;
                // Absolute-mode packed bytes are padded to a 16-bit boundary.
                if packed_len % 2 == 1 {
                    pos = pos
                        .checked_add(1)
                        .ok_or_else(|| ImageError::Decode("BI_RLE4 padding overflow".into()))?;
                    if pos > stream.len() {
                        return Err(ImageError::Decode("truncated BI_RLE4 absolute padding".into()));
                    }
                }
                x += n;
            }
        }
    }

    if !saw_eob {
        return Err(ImageError::Decode(
            "BI_RLE4 stream is missing end-of-bitmap marker".into(),
        ));
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

    fn rle4_bmp(width: i32, height: i32, palette: &[[u8; 4]], stream: &[u8]) -> Vec<u8> {
        let pixel_offset = 54 + palette.len() * 4;
        let mut out = vec![0u8; 54];
        out[0..2].copy_from_slice(b"BM");
        out[2..6].copy_from_slice(&((pixel_offset + stream.len()) as u32).to_le_bytes());
        out[10..14].copy_from_slice(&(pixel_offset as u32).to_le_bytes());
        out[14..18].copy_from_slice(&40u32.to_le_bytes());
        out[18..22].copy_from_slice(&width.to_le_bytes());
        out[22..26].copy_from_slice(&height.to_le_bytes());
        out[26..28].copy_from_slice(&1u16.to_le_bytes());
        out[28..30].copy_from_slice(&4u16.to_le_bytes());
        out[30..34].copy_from_slice(&2u32.to_le_bytes());
        out[34..38].copy_from_slice(&(stream.len() as u32).to_le_bytes());
        out[46..50].copy_from_slice(&(palette.len() as u32).to_le_bytes());
        for entry in palette {
            out.extend_from_slice(entry);
        }
        out.extend_from_slice(stream);
        out
    }

    #[test]
    fn decodes_encoded_and_absolute_nibbles_bottom_up() {
        let palette = [
            [0, 0, 0, 0],
            [0, 0, 255, 0],
            [0, 255, 0, 0],
            [255, 0, 0, 0],
        ];
        let bytes = rle4_bmp(
            5,
            2,
            &palette,
            &[
                5, 0x32, 0, 0,             // bottom: 3,2,3,2,3
                0, 5, 0x12, 0x12, 0x10, 0, // top absolute: 1,2,1,2,1 + pad
                0, 0, 0, 1,
            ],
        );
        let image = decode(&bytes).expect("RLE4 BMP");
        assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(image.pixel(1, 0), [0, 255, 0, 255]);
        assert_eq!(image.pixel(0, 1), [0, 0, 255, 255]);
        assert_eq!(image.pixel(1, 1), [0, 255, 0, 255]);
    }

    #[test]
    fn delta_keeps_palette_zero_background() {
        let palette = [[10, 20, 30, 0], [0, 0, 255, 0]];
        let bytes = rle4_bmp(3, 1, &palette, &[0, 2, 1, 0, 1, 0x10, 0, 1]);
        let image = decode(&bytes).expect("RLE4 delta");
        assert_eq!(image.pixel(0, 0), [30, 20, 10, 255]);
        assert_eq!(image.pixel(1, 0), [255, 0, 0, 255]);
        assert_eq!(image.pixel(2, 0), [30, 20, 10, 255]);
    }

    #[test]
    fn rejects_bad_absolute_padding_and_palette_indices() {
        let palette = [[0, 0, 0, 0], [0, 0, 255, 0]];
        assert!(decode(&rle4_bmp(3, 1, &palette, &[0, 3, 0x11, 0x10])).is_err());
        assert!(decode(&rle4_bmp(1, 1, &palette, &[1, 0x20, 0, 1])).is_err());
    }
}
