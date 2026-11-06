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

static FONTS: OnceLock<FontSet> = OnceLock::new();

struct LazyFont {
    path: PathBuf,
    loaded: OnceLock<Option<Font>>,
}

impl LazyFont {
    fn get(&self) -> Option<&Font> {
        self.loaded.get_or_init(|| load_font(&self.path)).as_ref()
    }
}

struct FontSet {
    primary: Option<Font>,
    fallbacks: Vec<LazyFont>,
}

impl FontSet {
    fn for_character(&self, character: char) -> Option<&Font> {
        if let Some(font) = &self.primary {
            if font.lookup_glyph_index(character) != 0 {
                return Some(font);
            }
        }
        // Only a missing glyph reaches the bounded fallback list. Parsed
        // fonts and failed paths are cached, so ASCII pages load no CJK font.
        for candidate in &self.fallbacks {
            if let Some(font) = candidate.get() {
                if font.lookup_glyph_index(character) != 0 {
                    return Some(font);
                }
            }
        }
        self.primary.as_ref().or_else(|| self.first_fallback())
    }

    fn first_fallback(&self) -> Option<&Font> {
        self.fallbacks.iter().find_map(LazyFont::get)
    }

    fn measure(&self, text: &str, font_size: f32) -> f32 {
        text.chars()
            .map(|character| {
                self.for_character(character)
                    .map(|font| font.metrics(character, font_size).advance_width)
                    .unwrap_or_else(|| fallback_advance(character, font_size))
            })
            .sum()
    }

    fn rasterize(&self, character: char, font_size: f32) -> Option<(Metrics, Vec<u8>)> {
        self.for_character(character)
            .map(|font| font.rasterize(character, font_size))
    }
}

pub fn measure_text(text: &str, font_size: f32) -> f32 {
    fonts().measure(text, font_size)
}

pub fn line_metrics(font_size: f32) -> LineMetrics {
    let fonts = fonts();
    fonts
        .primary
        .as_ref()
        .or_else(|| fonts.first_fallback())
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
    fonts().rasterize(character, font_size)
}

fn fonts() -> &'static FontSet {
    FONTS.get_or_init(|| FontSet {
        primary: font_candidates().iter().find_map(|path| load_font(path)),
        fallbacks: fallback_candidates()
            .into_iter()
            .map(|path| LazyFont {
                path,
                loaded: OnceLock::new(),
            })
            .collect(),
    })
}

fn load_font(path: &Path) -> Option<Font> {
    fs::read(path)
        .ok()
        .and_then(|bytes| Font::from_bytes(bytes, FontSettings::default()).ok())
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

fn fallback_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("BROWSER_ENGINE_FALLBACK_FONT") {
        candidates.push(PathBuf::from(path));
    }
    candidates.extend(
        [
            r"C:\Windows\Fonts\seguisym.ttf",
            r"C:\Windows\Fonts\seguiemj.ttf",
            r"C:\Windows\Fonts\msyh.ttc",
            r"C:\Windows\Fonts\mingliu.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
            "/System/Library/Fonts/PingFang.ttc",
        ]
        .into_iter()
        .map(PathBuf::from),
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

    fn fixture_fonts() -> FontSet {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fonts");
        FontSet {
            primary: load_font(&root.join("primary.ttf")),
            fallbacks: vec![LazyFont {
                path: root.join("fallback.ttf"),
                loaded: OnceLock::new(),
            }],
        }
    }

    #[test]
    fn primary_glyphs_do_not_load_fallbacks_or_change_metrics() {
        let fonts = fixture_fonts();
        assert_eq!(fonts.measure("AA", 20.0), 24.0);
        assert!(fonts.fallbacks[0].loaded.get().is_none());
        assert_eq!(fonts.rasterize('A', 20.0).unwrap().0.advance_width, 12.0);
        assert!(fonts.fallbacks[0].loaded.get().is_none());
    }

    #[test]
    fn missing_glyph_uses_the_same_fallback_for_measurement_and_pixels() {
        let fonts = fixture_fonts();
        assert_eq!(fonts.primary.as_ref().unwrap().lookup_glyph_index('中'), 0);
        let (metrics, bitmap) = fonts.rasterize('中', 20.0).unwrap();
        assert!(bitmap.iter().any(|value| *value != 0));
        assert_eq!(metrics.advance_width, 18.0);
        assert_eq!(
            fonts.measure("A中A", 20.0),
            12.0 + metrics.advance_width + 12.0
        );
        // A missing glyph in every candidate retains the primary's .notdef.
        assert!(std::ptr::eq(
            fonts.for_character('Ω').unwrap(),
            fonts.primary.as_ref().unwrap()
        ));
    }
}
