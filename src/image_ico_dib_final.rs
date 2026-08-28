// ============================================================
// image_ico_dib_final.rs — traditional DIB-backed ICO decoding
// ============================================================

use std::collections::HashMap;
use std::rc::Rc;

use crate::net::{LoadError, ResourceLoader, Url};

pub use crate::image_prev15::{ImageError, ImageFormat, RasterImage};

/// Decode image bytes into straight RGBA8, adding traditional 32-bit
/// DIB-backed Windows ICO entries on top of the PNG-backed ICO layer.
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
        // Reuse the previous layer so PNG dimension validation remains in one place.
        return crate::image_prev15::decode(bytes);
    }

    decode_dib32(payload, selected.width, selected.height, selected.bit_depth)
}

fn decode_dib32(
    payload: &[u8],
    directory_width: u32,
    directory_height: u32,
    directory_depth: u16,
) -> Result<RasterImage, ImageError> {
    let header_size = usize::try_from(read_u32(payload, 0, "DIB header size")?)
        .map_err(|_| ImageError::Decode("ICO DIB header size does not fit this platform".into()))?;
    if header_size < 40 {
        return Err(ImageError::Decode(
            "ICO DIB entry requires a BITMAPINFOHEADER or later header".into(),
        ));
    }
    if header_size > payload.len() {
        return Err(ImageError::Decode("truncated ICO DIB header".into()));
    }

    let width_i = read_i32(payload, 4, "DIB width")?;
    let doubled_height_i = read_i32(payload, 8, "DIB height")?;
    if width_i <= 0 || doubled_height_i <= 0 || doubled_height_i % 2 != 0 {
        return Err(ImageError::Decode(
            "ICO DIB dimensions must be positive and height must include XOR+AND planes".into(),
        ));
    }
    let width = width_i as u32;
    let height = (doubled_height_i / 2) as u32;
    if width != directory_width || height != directory_height {
        return Err(ImageError::Decode(format!(
            "ICO directory dimensions {}x{} do not match DIB payload {}x{}",
            directory_width, directory_height, width, height
        )));
    }

    let planes = read_u16(payload, 12, "DIB planes")?;
    let bit_depth = read_u16(payload, 14, "DIB bit depth")?;
    let compression = read_u32(payload, 16, "DIB compression")?;
    if planes != 1 {
        return Err(ImageError::Decode("ICO DIB planes must equal 1".into()));
    }
    if bit_depth != 32 || directory_depth != 32 {
        return Err(ImageError::Decode(
            "this ICO DIB layer currently supports 32-bit entries only".into(),
        ));
    }
    if compression != 0 {
        return Err(ImageError::Decode(
            "compressed 32-bit ICO DIB entries are not supported".into(),
        ));
    }

    let width_usize = usize::try_from(width)
        .map_err(|_| ImageError::Decode("ICO DIB width does not fit this platform".into()))?;
    let height_usize = usize::try_from(height)
        .map_err(|_| ImageError::Decode("ICO DIB height does not fit this platform".into()))?;
    let xor_stride = width_usize
        .checked_mul(4)
        .ok_or_else(|| ImageError::Decode("ICO DIB XOR row size overflow".into()))?;
    let xor_size = xor_stride
        .checked_mul(height_usize)
        .ok_or_else(|| ImageError::Decode("ICO DIB XOR raster size overflow".into()))?;
    let and_stride = width_usize
        .checked_add(31)
        .ok_or_else(|| ImageError::Decode("ICO DIB AND row size overflow".into()))?
        / 32
        * 4;
    let and_size = and_stride
        .checked_mul(height_usize)
        .ok_or_else(|| ImageError::Decode("ICO DIB AND raster size overflow".into()))?;
    let xor_start = header_size;
    let and_start = xor_start
        .checked_add(xor_size)
        .ok_or_else(|| ImageError::Decode("ICO DIB raster offset overflow".into()))?;
    let required = and_start
        .checked_add(and_size)
        .ok_or_else(|| ImageError::Decode("ICO DIB raster size overflow".into()))?;
    if required > payload.len() {
        return Err(ImageError::Decode("truncated ICO DIB XOR/AND raster".into()));
    }

    let rgba_len = width_usize
        .checked_mul(height_usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| ImageError::Decode("ICO DIB RGBA size overflow".into()))?;
    let mut pixels = vec![0u8; rgba_len];
    let mut any_nonzero_alpha = false;

    for out_y in 0..height_usize {
        let src_y = height_usize - 1 - out_y;
        let row = xor_start + src_y * xor_stride;
        for x in 0..width_usize {
            let src = row + x * 4;
            let dst = (out_y * width_usize + x) * 4;
            let b = payload[src];
            let g = payload[src + 1];
            let r = payload[src + 2];
            let a = payload[src + 3];
            any_nonzero_alpha |= a != 0;
            pixels[dst..dst + 4].copy_from_slice(&[r, g, b, a]);
        }
    }

    // Legacy 32-bit icons frequently leave the alpha byte at zero and rely on
    // the 1-bit AND mask. Only synthesize alpha in that legacy case; when any
    // XOR alpha is present, preserve the authored alpha channel.
    if !any_nonzero_alpha {
        for out_y in 0..height_usize {
            let src_y = height_usize - 1 - out_y;
            let row = and_start + src_y * and_stride;
            for x in 0..width_usize {
                let mask = (payload[row + x / 8] >> (7 - (x % 8))) & 1;
                let alpha = if mask == 0 { 255 } else { 0 };
                pixels[(out_y * width_usize + x) * 4 + 3] = alpha;
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

fn load_and_decode(
    url: &Url,
    loader: &dyn ResourceLoader,
) -> Result<Rc<RasterImage>, String> {
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

    fn ico_dib32(width: u8, height: u8, pixels_top_down: &[[u8; 4]], and_rows: &[u8]) -> Vec<u8> {
        let w = width as usize;
        let h = height as usize;
        assert_eq!(pixels_top_down.len(), w * h);
        let and_stride = ((w + 31) / 32) * 4;
        assert_eq!(and_rows.len(), and_stride * h);

        let mut dib = vec![0u8; 40];
        dib[0..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&(width as i32).to_le_bytes());
        dib[8..12].copy_from_slice(&((height as i32) * 2).to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        for y in (0..h).rev() {
            for x in 0..w {
                let [r, g, b, a] = pixels_top_down[y * w + x];
                dib.extend_from_slice(&[b, g, r, a]);
            }
        }
        dib.extend_from_slice(and_rows);

        let mut out = vec![0u8; 22];
        out[2..4].copy_from_slice(&1u16.to_le_bytes());
        out[4..6].copy_from_slice(&1u16.to_le_bytes());
        out[6] = width;
        out[7] = height;
        out[10..12].copy_from_slice(&1u16.to_le_bytes());
        out[12..14].copy_from_slice(&32u16.to_le_bytes());
        out[14..18].copy_from_slice(&(dib.len() as u32).to_le_bytes());
        out[18..22].copy_from_slice(&22u32.to_le_bytes());
        out.extend_from_slice(&dib);
        out
    }

    #[test]
    fn decodes_dib32_and_flips_bottom_up_rows() {
        let bytes = ico_dib32(
            2,
            2,
            &[
                [255, 0, 0, 255],
                [0, 255, 0, 128],
                [0, 0, 255, 64],
                [255, 255, 0, 32],
            ],
            &[0; 8],
        );
        let image = decode(&bytes).unwrap();
        assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(image.pixel(1, 0), [0, 255, 0, 128]);
        assert_eq!(image.pixel(0, 1), [0, 0, 255, 64]);
    }

    #[test]
    fn uses_and_mask_when_xor_alpha_is_all_zero() {
        let mut mask = vec![0u8; 4];
        mask[0] = 0b0100_0000; // x=1 transparent; x=0 opaque.
        let bytes = ico_dib32(
            2,
            1,
            &[[10, 20, 30, 0], [40, 50, 60, 0]],
            &mask,
        );
        let image = decode(&bytes).unwrap();
        assert_eq!(image.pixel(0, 0), [10, 20, 30, 255]);
        assert_eq!(image.pixel(1, 0), [40, 50, 60, 0]);
    }

    #[test]
    fn preserves_authored_alpha_instead_of_and_mask() {
        let mut mask = vec![0u8; 4];
        mask[0] = 0b1000_0000;
        let bytes = ico_dib32(1, 1, &[[1, 2, 3, 77]], &mask);
        let image = decode(&bytes).unwrap();
        assert_eq!(image.pixel(0, 0), [1, 2, 3, 77]);
    }

    #[test]
    fn rejects_malformed_dib_dimensions_and_raster() {
        let mut bytes = ico_dib32(1, 1, &[[1, 2, 3, 4]], &[0; 4]);
        let payload = 22;
        bytes[payload + 8..payload + 12].copy_from_slice(&3i32.to_le_bytes());
        assert!(decode(&bytes).is_err());

        let mut bytes = ico_dib32(1, 1, &[[1, 2, 3, 4]], &[0; 4]);
        bytes.truncate(bytes.len() - 1);
        bytes[14..18].copy_from_slice(&((bytes.len() - 22) as u32).to_le_bytes());
        assert!(decode(&bytes).is_err());
    }
}
