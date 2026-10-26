// ============================================================
//  tab_strip.rs  —  row of tabs above the toolbar for --window
// ============================================================
//
//  Like the toolbar, the tab strip is painted with the engine's own painter
//  onto a strip of its own. It sits above the toolbar, and the window grows
//  by `TAB_STRIP_HEIGHT`, so the page keeps its full `--size` viewport.
//
//  This file owns the strip's geometry, its pointer handling and its paint,
//  plus `route_window`, which splits a window point between the strip, the
//  toolbar and the page. It never touches a `Browser` or a `Tab`: it hands
//  `main` a `TabCommand` to carry out.

use browser_engine::{
    css::parser::Color,
    layout::{Rect, TextFragment},
    paint::{Canvas, DisplayCommand},
    text::{line_metrics, measure_text},
};

use crate::tabs::TabCommand;
use crate::toolbar::{
    contains, distance_to, stroke, PointerTarget, Segment, Toolbar, BAR, ICON, ICON_DISABLED, TEXT,
};

/// Height of the tab strip in pixels.
pub(crate) const TAB_STRIP_HEIGHT: usize = 32;

const MARGIN: f32 = 6.0;
/// Gap above the tabs; the active tab runs down into the toolbar.
const TAB_TOP: f32 = 4.0;
const MAX_TAB_WIDTH: f32 = 220.0;
/// Narrowest a tab may get. The strip refuses more tabs than fit at this
/// width rather than scrolling or squeezing them past readability.
pub(crate) const MIN_TAB_WIDTH: f32 = 72.0;
const NEW_TAB_SIZE: f32 = 24.0;
const NEW_TAB_GAP: f32 = 4.0;
const CLOSE_SIZE: f32 = 16.0;
const LABEL_PADDING: f32 = 10.0;
const FONT_SIZE: f32 = 13.0;
const ELLIPSIS: char = '…';

const STRIP: Color = Color::rgb(222, 225, 230);
const TAB_HOVER: Color = Color::rgb(232, 234, 237);
const CONTROL_HOVER: Color = Color::rgb(206, 209, 214);
const SEPARATOR: Color = Color::rgb(176, 180, 186);
const TEXT_INACTIVE: Color = Color::rgb(84, 88, 94);

/// A clickable part of the strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TabControl {
    Tab(usize),
    Close(usize),
    NewTab,
}

/// Where a window point lands, outermost chrome first.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum WindowPoint {
    /// Inside the tab strip, in strip coordinates (the same as the window's).
    TabStrip {
        x: f32,
        y: f32,
    },
    /// Inside the toolbar, in toolbar coordinates.
    Toolbar {
        x: f32,
        y: f32,
    },
    /// Inside the page viewport, in document coordinates.
    Page {
        x: f32,
        y: f32,
    },
    Outside,
}

/// Route a window point through the tab strip, then the active tab's
/// toolbar, then its page viewport and scroll offset.
pub(crate) fn route_window(
    toolbar: &Toolbar,
    x: f32,
    y: f32,
    page_height: usize,
    scroll_y: f32,
) -> WindowPoint {
    if y < 0.0 {
        return WindowPoint::Outside;
    }
    if y < TAB_STRIP_HEIGHT as f32 {
        // The toolbar knows the window's width; let it reject x for us.
        return match toolbar.route(x, 0.0, page_height, 0.0) {
            PointerTarget::Outside => WindowPoint::Outside,
            _ => WindowPoint::TabStrip { x, y },
        };
    }
    let below = y - TAB_STRIP_HEIGHT as f32;
    match toolbar.route(x, below, page_height, scroll_y) {
        PointerTarget::Toolbar => WindowPoint::Toolbar { x, y: below },
        PointerTarget::Page { x, y } => WindowPoint::Page { x, y },
        PointerTarget::Outside => WindowPoint::Outside,
    }
}

pub(crate) struct TabStrip {
    width: usize,
    hovered: Option<TabControl>,
    /// Close or new-tab button armed by a left press.
    pressed: Option<TabControl>,
    /// Tab armed by a middle press; it closes if the release lands on it.
    middle_pressed: Option<usize>,
}

impl TabStrip {
    pub(crate) fn new(width: usize) -> TabStrip {
        TabStrip {
            width,
            hovered: None,
            pressed: None,
            middle_pressed: None,
        }
    }

    // ── Geometry ──────────────────────────────────────────────────────────

    /// Width shared by the tabs and the new-tab button.
    fn available(&self) -> f32 {
        (self.width as f32 - 2.0 * MARGIN - NEW_TAB_GAP - NEW_TAB_SIZE).max(0.0)
    }

    /// How many tabs fit at `MIN_TAB_WIDTH`. Always at least one.
    pub(crate) fn capacity(&self) -> usize {
        ((self.available() / MIN_TAB_WIDTH) as usize).max(1)
    }

    fn tab_width(&self, count: usize) -> f32 {
        (self.available() / count.max(1) as f32).min(MAX_TAB_WIDTH)
    }

    pub(crate) fn tab_rect(&self, index: usize, count: usize) -> Rect {
        let width = self.tab_width(count);
        Rect {
            x: MARGIN + index as f32 * width,
            y: TAB_TOP,
            width,
            height: TAB_STRIP_HEIGHT as f32 - TAB_TOP,
        }
    }

    pub(crate) fn close_rect(&self, index: usize, count: usize) -> Rect {
        let tab = self.tab_rect(index, count);
        Rect {
            x: tab.x + tab.width - LABEL_PADDING / 2.0 - CLOSE_SIZE,
            y: tab.y + (tab.height - CLOSE_SIZE) / 2.0,
            width: CLOSE_SIZE,
            height: CLOSE_SIZE,
        }
    }

    pub(crate) fn new_tab_rect(&self, count: usize) -> Rect {
        let last = self.tab_rect(count.max(1) - 1, count);
        Rect {
            x: last.x + last.width + NEW_TAB_GAP,
            y: TAB_TOP + (TAB_STRIP_HEIGHT as f32 - TAB_TOP - NEW_TAB_SIZE) / 2.0,
            width: NEW_TAB_SIZE,
            height: NEW_TAB_SIZE,
        }
    }

    /// The control under a strip point. A tab's close button wins over the
    /// tab around it.
    pub(crate) fn control_at(&self, x: f32, y: f32, count: usize) -> Option<TabControl> {
        if contains(self.new_tab_rect(count), x, y) {
            return Some(TabControl::NewTab);
        }
        (0..count).find_map(|index| {
            if contains(self.close_rect(index, count), x, y) {
                Some(TabControl::Close(index))
            } else if contains(self.tab_rect(index, count), x, y) {
                Some(TabControl::Tab(index))
            } else {
                None
            }
        })
    }

    // ── Pointer ───────────────────────────────────────────────────────────

    /// Track hover. `x`/`y` may be anywhere in the window.
    pub(crate) fn pointer_moved(&mut self, x: f32, y: f32, count: usize) {
        self.hovered = self.control_at(x, y, count);
    }

    pub(crate) fn pointer_left(&mut self) {
        self.hovered = None;
    }

    /// A left press in the strip. A tab is selected as soon as it is
    /// pressed, as in other browsers; buttons wait for the release.
    pub(crate) fn press(&mut self, x: f32, y: f32, count: usize) -> Option<TabCommand> {
        match self.control_at(x, y, count)? {
            TabControl::Tab(index) => Some(TabCommand::Select(index)),
            control => {
                self.pressed = Some(control);
                None
            }
        }
    }

    /// The left button came up anywhere. A button fires only if it was
    /// pressed and the release lands on it too.
    pub(crate) fn release(&mut self, x: f32, y: f32, count: usize) -> Option<TabCommand> {
        let armed = self.pressed.take()?;
        if self.control_at(x, y, count) != Some(armed) {
            return None;
        }
        match armed {
            TabControl::Close(index) => Some(TabCommand::Close(index)),
            TabControl::NewTab => Some(TabCommand::New),
            TabControl::Tab(_) => None,
        }
    }

    /// A middle press in the strip arms the tab under it.
    pub(crate) fn press_middle(&mut self, x: f32, y: f32, count: usize) {
        self.middle_pressed = match self.control_at(x, y, count) {
            Some(TabControl::Tab(index) | TabControl::Close(index)) => Some(index),
            _ => None,
        };
    }

    /// The middle button came up: close the tab if it was pressed there.
    pub(crate) fn release_middle(&mut self, x: f32, y: f32, count: usize) -> Option<TabCommand> {
        let armed = self.middle_pressed.take()?;
        match self.control_at(x, y, count) {
            Some(TabControl::Tab(index) | TabControl::Close(index)) if index == armed => {
                Some(TabCommand::Close(index))
            }
            _ => None,
        }
    }

    // ── Paint ─────────────────────────────────────────────────────────────

    /// Draw the strip for tabs labelled `labels`, with `active` selected.
    pub(crate) fn paint(&self, labels: &[String], active: usize) -> Canvas {
        let count = labels.len();
        let mut canvas = Canvas::new(self.width, TAB_STRIP_HEIGHT, STRIP);

        for (index, label) in labels.iter().enumerate() {
            let rect = self.tab_rect(index, count);
            let is_active = index == active;
            let fill = if is_active {
                Some(BAR)
            } else if matches!(self.hovered, Some(TabControl::Tab(i) | TabControl::Close(i)) if i == index)
            {
                Some(TAB_HOVER)
            } else {
                None
            };
            if let Some(fill) = fill {
                // Round the top corners only, so the active tab runs
                // seamlessly into the toolbar below it.
                canvas.paint(&DisplayCommand::RoundedRect(fill, rect, 7.0));
                canvas.paint(&DisplayCommand::SolidColor(
                    fill,
                    Rect {
                        y: rect.y + rect.height / 2.0,
                        height: rect.height / 2.0,
                        ..rect
                    },
                ));
            }
            // Separate neighbouring tabs that are both plain.
            let next_plain = index + 1 < count && index + 1 != active;
            if !is_active && fill.is_none() && next_plain {
                canvas.paint(&DisplayCommand::SolidColor(
                    SEPARATOR,
                    Rect {
                        x: (rect.x + rect.width).round() - 1.0,
                        y: rect.y + 7.0,
                        width: 1.0,
                        height: rect.height - 14.0,
                    },
                ));
            }
            self.paint_label(&mut canvas, index, count, label, is_active);
            self.paint_close(&mut canvas, index, count, is_active);
        }
        self.paint_new_tab(&mut canvas, count);
        canvas
    }

    fn paint_label(
        &self,
        canvas: &mut Canvas,
        index: usize,
        count: usize,
        label: &str,
        active: bool,
    ) {
        let tab = self.tab_rect(index, count);
        let left = tab.x + LABEL_PADDING;
        let right = self.close_rect(index, count).x - 4.0;
        if right <= left {
            return;
        }
        let text = fit_label(label, right - left);
        let metrics = line_metrics(FONT_SIZE);
        let baseline =
            (tab.y + (tab.height - metrics.new_line_size) / 2.0 + metrics.ascent).round();
        let line_top = baseline - metrics.ascent;
        canvas.paint(&DisplayCommand::PushClip(Rect {
            x: left,
            y: tab.y,
            width: right - left,
            height: tab.height,
        }));
        canvas.paint(&DisplayCommand::Text(TextFragment {
            rect: Rect {
                x: left,
                y: line_top,
                width: measure_text(&text, FONT_SIZE),
                height: metrics.new_line_size,
            },
            text,
            baseline,
            color: if active { TEXT } else { TEXT_INACTIVE },
            font_size: FONT_SIZE,
            underline: false,
            strikethrough: false,
            text_shadow: None,
        }));
        canvas.paint(&DisplayCommand::PopClip);
    }

    fn paint_close(&self, canvas: &mut Canvas, index: usize, count: usize, active: bool) {
        let rect = self.close_rect(index, count);
        if self.hovered == Some(TabControl::Close(index)) {
            canvas.paint(&DisplayCommand::RoundedRect(CONTROL_HOVER, rect, 4.0));
        }
        let (cx, cy) = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
        let cross = [
            Segment {
                from: (cx - 3.5, cy - 3.5),
                to: (cx + 3.5, cy + 3.5),
            },
            Segment {
                from: (cx - 3.5, cy + 3.5),
                to: (cx + 3.5, cy - 3.5),
            },
        ];
        let color = if active { ICON } else { TEXT_INACTIVE };
        stroke(canvas, rect, color, |x, y| distance_to(&cross, x, y));
    }

    fn paint_new_tab(&self, canvas: &mut Canvas, count: usize) {
        let rect = self.new_tab_rect(count);
        let enabled = count < self.capacity();
        if enabled && self.hovered == Some(TabControl::NewTab) {
            canvas.paint(&DisplayCommand::RoundedRect(CONTROL_HOVER, rect, 6.0));
        }
        let (cx, cy) = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
        let plus = [
            Segment {
                from: (cx - 6.0, cy),
                to: (cx + 6.0, cy),
            },
            Segment {
                from: (cx, cy - 6.0),
                to: (cx, cy + 6.0),
            },
        ];
        let color = if enabled { ICON } else { ICON_DISABLED };
        stroke(canvas, rect, color, |x, y| distance_to(&plus, x, y));
    }
}

/// `label` on one line, cut with an ellipsis to fit `max_width` pixels.
///
/// Page titles can hold newlines, tabs and runs of spaces; they collapse to
/// single spaces so the label stays on its line.
pub(crate) fn fit_label(label: &str, max_width: f32) -> String {
    let clean = label
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if measure_text(&clean, FONT_SIZE) <= max_width {
        return clean;
    }
    let budget = max_width - measure_text(&ELLIPSIS.to_string(), FONT_SIZE);
    let mut fitted = String::new();
    let mut width = 0.0;
    for character in clean.chars() {
        let advance = measure_text(character.encode_utf8(&mut [0; 4]), FONT_SIZE);
        if width + advance > budget {
            break;
        }
        width += advance;
        fitted.push(character);
    }
    let mut fitted = fitted.trim_end().to_string();
    fitted.push(ELLIPSIS);
    fitted
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toolbar::{Control, TOOLBAR_HEIGHT};

    const WIDTH: usize = 800;
    const PAGE_HEIGHT: usize = 600;

    fn center(rect: Rect) -> (f32, f32) {
        (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0)
    }

    fn pixel(canvas: &Canvas, x: f32, y: f32) -> Color {
        let i = (y as usize * canvas.width + x as usize) * 3;
        Color::rgb(canvas.pixels[i], canvas.pixels[i + 1], canvas.pixels[i + 2])
    }

    // ── Geometry ──────────────────────────────────────────────────────────

    #[test]
    fn tabs_sit_left_to_right_inside_the_strip_with_the_new_tab_button_last() {
        let strip = TabStrip::new(WIDTH);
        for count in 1..=strip.capacity() {
            let mut previous_right = 0.0;
            for index in 0..count {
                let tab = strip.tab_rect(index, count);
                assert!(
                    tab.x >= previous_right - 0.01,
                    "tab {index}/{count} overlaps"
                );
                assert!(tab.width >= MIN_TAB_WIDTH && tab.width <= MAX_TAB_WIDTH);
                assert!(tab.y >= 0.0 && tab.y + tab.height <= TAB_STRIP_HEIGHT as f32);
                let close = strip.close_rect(index, count);
                assert!(close.x > tab.x && close.x + close.width <= tab.x + tab.width);
                previous_right = tab.x + tab.width;
            }
            let plus = strip.new_tab_rect(count);
            assert!(plus.x >= previous_right - 0.01);
            assert!(plus.x + plus.width <= WIDTH as f32, "{count} tabs: + fits");
        }
    }

    #[test]
    fn tabs_shrink_as_more_open_and_capacity_keeps_them_readable() {
        let strip = TabStrip::new(WIDTH);
        assert_eq!(strip.tab_rect(0, 1).width, MAX_TAB_WIDTH);
        assert!(strip.tab_rect(0, 8).width < strip.tab_rect(0, 3).width);
        let capacity = strip.capacity();
        assert!(
            capacity >= 8,
            "an 800px window holds a useful number of tabs"
        );
        assert!(strip.tab_rect(0, capacity).width >= MIN_TAB_WIDTH);
        assert!(strip.tab_rect(0, capacity + 1).width < MIN_TAB_WIDTH);
        // A window too narrow for even one tab still has room for one.
        assert_eq!(TabStrip::new(40).capacity(), 1);
    }

    #[test]
    fn control_at_finds_tabs_close_buttons_and_the_new_tab_button() {
        let strip = TabStrip::new(WIDTH);
        for index in 0..3 {
            let (x, y) = center(strip.tab_rect(index, 3));
            assert_eq!(strip.control_at(x, y, 3), Some(TabControl::Tab(index)));
            let (x, y) = center(strip.close_rect(index, 3));
            assert_eq!(strip.control_at(x, y, 3), Some(TabControl::Close(index)));
        }
        let (x, y) = center(strip.new_tab_rect(3));
        assert_eq!(strip.control_at(x, y, 3), Some(TabControl::NewTab));
        assert_eq!(strip.control_at(1.0, 1.0, 3), None);
        assert_eq!(strip.control_at(WIDTH as f32 - 2.0, 16.0, 3), None);
    }

    // ── Pointer ───────────────────────────────────────────────────────────

    #[test]
    fn pressing_a_tab_selects_it_immediately() {
        let mut strip = TabStrip::new(WIDTH);
        let (x, y) = center(strip.tab_rect(1, 3));
        assert_eq!(strip.press(x, y, 3), Some(TabCommand::Select(1)));
        assert_eq!(strip.release(x, y, 3), None, "the release adds nothing");
    }

    #[test]
    fn close_and_new_tab_fire_on_release_over_the_same_button() {
        let mut strip = TabStrip::new(WIDTH);
        let (cx, cy) = center(strip.close_rect(2, 3));
        assert_eq!(strip.press(cx, cy, 3), None);
        assert_eq!(strip.release(cx, cy, 3), Some(TabCommand::Close(2)));

        let (px, py) = center(strip.new_tab_rect(3));
        assert_eq!(strip.press(px, py, 3), None);
        assert_eq!(strip.release(px, py, 3), Some(TabCommand::New));

        // Pressed on a close button, released elsewhere: cancelled.
        strip.press(cx, cy, 3);
        assert_eq!(strip.release(px, py, 3), None);
        // Released after the tab count changed under the pointer: the button
        // there is now a different control, so nothing fires.
        strip.press(cx, cy, 3);
        assert_eq!(strip.release(cx, cy, 2), None);
        // A release with no press is nothing.
        assert_eq!(strip.release(cx, cy, 3), None);
    }

    #[test]
    fn middle_click_closes_the_tab_it_started_and_ended_on() {
        let mut strip = TabStrip::new(WIDTH);
        let (x, y) = center(strip.tab_rect(1, 3));
        strip.press_middle(x, y, 3);
        assert_eq!(strip.release_middle(x, y, 3), Some(TabCommand::Close(1)));

        let (other, _) = center(strip.tab_rect(0, 3));
        strip.press_middle(x, y, 3);
        assert_eq!(strip.release_middle(other, y, 3), None);
        let (px, py) = center(strip.new_tab_rect(3));
        strip.press_middle(px, py, 3);
        assert_eq!(strip.release_middle(px, py, 3), None);
    }

    // ── Window routing ────────────────────────────────────────────────────

    #[test]
    fn window_points_route_through_strip_then_toolbar_then_page() {
        let toolbar = Toolbar::new(WIDTH);
        let route = |x, y, scroll| route_window(&toolbar, x, y, PAGE_HEIGHT, scroll);
        let strip = TAB_STRIP_HEIGHT as f32;
        let bar = TOOLBAR_HEIGHT as f32;

        assert_eq!(
            route(10.0, 0.0, 0.0),
            WindowPoint::TabStrip { x: 10.0, y: 0.0 }
        );
        assert_eq!(
            route(10.0, strip - 0.5, 0.0),
            WindowPoint::TabStrip {
                x: 10.0,
                y: strip - 0.5
            }
        );
        // The toolbar's own coordinates start below the strip.
        assert_eq!(
            route(10.0, strip, 0.0),
            WindowPoint::Toolbar { x: 10.0, y: 0.0 }
        );
        let (bx, by) = center(toolbar.rect(Control::Back));
        match route(bx, by + strip, 0.0) {
            WindowPoint::Toolbar { x, y } => {
                assert_eq!(toolbar.control_at(x, y), Some(Control::Back));
            }
            other => panic!("expected the Back button, got {other:?}"),
        }
        // The first page row is below both strips, and scroll is added.
        assert_eq!(
            route(10.0, strip + bar, 0.0),
            WindowPoint::Page { x: 10.0, y: 0.0 }
        );
        assert_eq!(
            route(25.0, strip + bar + 100.0, 250.0),
            WindowPoint::Page { x: 25.0, y: 350.0 }
        );
        let bottom = strip + bar + PAGE_HEIGHT as f32;
        assert_eq!(
            route(10.0, bottom - 1.0, 0.0),
            WindowPoint::Page {
                x: 10.0,
                y: PAGE_HEIGHT as f32 - 1.0
            }
        );
        assert_eq!(route(10.0, bottom, 0.0), WindowPoint::Outside);
        assert_eq!(route(-1.0, 10.0, 0.0), WindowPoint::Outside);
        assert_eq!(route(WIDTH as f32, 10.0, 0.0), WindowPoint::Outside);
        assert_eq!(route(10.0, -1.0, 0.0), WindowPoint::Outside);
    }

    #[test]
    fn no_point_on_a_tab_reaches_the_page_even_when_scrolled() {
        let toolbar = Toolbar::new(WIDTH);
        let strip = TabStrip::new(WIDTH);
        for index in 0..4 {
            let tab = strip.tab_rect(index, 4);
            for y in [0.0, tab.y, tab.y + tab.height - 0.5] {
                for x in [tab.x, tab.x + tab.width / 2.0] {
                    assert!(
                        matches!(
                            route_window(&toolbar, x, y, PAGE_HEIGHT, 900.0),
                            WindowPoint::TabStrip { .. }
                        ),
                        "({x},{y})"
                    );
                }
            }
        }
    }

    // ── Labels ────────────────────────────────────────────────────────────

    #[test]
    fn labels_that_fit_are_kept_and_long_ones_end_in_an_ellipsis() {
        assert_eq!(fit_label("Home", 500.0), "Home");
        assert_eq!(fit_label("  A\n\tlong\r\n  title ", 500.0), "A long title");
        let long = "An extremely long page title that cannot possibly fit".repeat(3);
        let fitted = fit_label(&long, 120.0);
        assert!(fitted.ends_with(ELLIPSIS), "{fitted:?}");
        assert!(
            measure_text(&fitted, FONT_SIZE) <= 120.0 + 0.01,
            "{fitted:?}"
        );
        assert!(long.starts_with(fitted.trim_end_matches(ELLIPSIS)));
        // Multibyte text is cut on a char boundary, never mid code point.
        let wide = "標題".repeat(40);
        let fitted = fit_label(&wide, 60.0);
        assert!(fitted.ends_with(ELLIPSIS));
        assert!(wide.starts_with(fitted.trim_end_matches(ELLIPSIS)));
    }

    // ── Paint ─────────────────────────────────────────────────────────────

    #[test]
    fn the_active_tab_matches_the_toolbar_and_others_do_not() {
        let strip = TabStrip::new(WIDTH);
        let labels = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let canvas = strip.paint(&labels, 1);
        assert_eq!((canvas.width, canvas.height), (WIDTH, TAB_STRIP_HEIGHT));
        let bottom = TAB_STRIP_HEIGHT as f32 - 1.0;
        let probe = |index| {
            let tab = strip.tab_rect(index, 3);
            pixel(&canvas, tab.x + 4.0, bottom)
        };
        assert_eq!(probe(1), BAR, "active tab merges into the toolbar");
        assert_eq!(probe(0), STRIP);
        assert_eq!(probe(2), STRIP);
        assert_eq!(pixel(&canvas, 1.0, 1.0), STRIP);

        let canvas = strip.paint(&labels, 2);
        let tab = strip.tab_rect(2, 3);
        assert_eq!(pixel(&canvas, tab.x + 4.0, bottom), BAR);
    }

    #[test]
    fn new_tab_button_is_drawn_disabled_at_capacity() {
        let strip = TabStrip::new(WIDTH);
        let darkest = |labels: &[String]| {
            let canvas = strip.paint(labels, 0);
            let rect = strip.new_tab_rect(labels.len());
            let mut min = 255u8;
            for y in rect.y as usize..(rect.y + rect.height) as usize {
                for x in rect.x as usize..(rect.x + rect.width) as usize {
                    min = min.min(pixel(&canvas, x as f32, y as f32).r);
                }
            }
            min
        };
        let few = vec![String::new(); 2];
        let full = vec![String::new(); strip.capacity()];
        assert_eq!(darkest(&few), ICON.r);
        assert_eq!(darkest(&full), ICON_DISABLED.r);
    }

    #[test]
    fn labels_are_drawn_when_a_font_is_available() {
        if browser_engine::text::rasterize('h', FONT_SIZE).is_none() {
            return; // headless machine without a system font
        }
        let strip = TabStrip::new(WIDTH);
        let tab = strip.tab_rect(0, 1);
        let label_area = |canvas: &Canvas| {
            let mut dark = 0;
            for y in tab.y as usize..(tab.y + tab.height) as usize {
                for x in (tab.x + LABEL_PADDING) as usize..strip.close_rect(0, 1).x as usize {
                    if pixel(canvas, x as f32, y as f32).r < 128 {
                        dark += 1;
                    }
                }
            }
            dark
        };
        assert_eq!(label_area(&strip.paint(&[String::new()], 0)), 0);
        assert!(label_area(&strip.paint(&["Example page".into()], 0)) > 30);
    }
}
