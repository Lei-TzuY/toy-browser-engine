use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use fontdue::{Font, FontSettings, Metrics};

#[derive(Debug, Clone, Copy)]
pub struct LineMetrics {
    pub ascent: f32,
    pub new_line_size: f32,
}

static DEFAULT_FONT: OnceLock<Option<Font>> = OnceLock::new();

pub fn measure_text(text: &str, font_size: f32) -> f32 {
    match default_font() {
        Some(font) => text
            .chars()
            .map(|character| font.metrics(character, font_size).advance_width)
            .sum(),
        None => text
            .chars()
            .map(|character| fallback_advance(character, font_size))
            .sum(),
    }
}

pub fn line_metrics(font_size: f32) -> LineMetrics {
    default_font()
        .and_then(|font| font.horizontal_line_metrics(font_size))
        .map(|metrics| LineMetrics {
            ascent: metrics.ascent,
            new_line_size: metrics.new_line_size,
        })
        .unwrap_or(LineMetrics {
            ascent: font_size * 0.8,
            new_line_size: font_size * 1.2,
        })
}

pub fn rasterize(character: char, font_size: f32) -> Option<(Metrics, Vec<u8>)> {
    default_font().map(|font| font.rasterize(character, font_size))
}

fn default_font() -> Option<&'static Font> {
    DEFAULT_FONT.get_or_init(load_default_font).as_ref()
}

fn load_default_font() -> Option<Font> {
    font_candidates().into_iter().find_map(|path| {
        fs::read(path)
            .ok()
            .and_then(|bytes| Font::from_bytes(bytes, FontSettings::default()).ok())
    })
}

fn font_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("BROWSER_ENGINE_FONT") {
        candidates.push(PathBuf::from(path));
    }

    candidates.extend(
        [
            r"C:\Windows\Fonts\segoeui.ttf",
            r"C:\Windows\Fonts\arial.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/liberation2/LiberationSans-Regular.ttf",
            "/System/Library/Fonts/Supplemental/Arial.ttf",
        ]
        .into_iter()
        .map(Path::new)
        .map(Path::to_path_buf),
    );
    candidates
}

fn fallback_advance(character: char, font_size: f32) -> f32 {
    if character.is_whitespace() {
        font_size * 0.33
    } else {
        font_size * 0.6
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_measurement_is_positive() {
        assert!(measure_text("hello", 16.0) > 0.0);
    }

    #[test]
    fn line_height_is_positive() {
        assert!(line_metrics(16.0).new_line_size > 0.0);
    }
}
