// ============================================================
//  toolbar.rs  —  on-canvas browser toolbar for --window
// ============================================================
//
//  minifb gives us a pixel buffer and raw input, nothing more, so the
//  toolbar is drawn with the engine's own painter onto a strip that sits
//  above the page. The page keeps its full `--size` viewport; the window is
//  simply taller by `TOOLBAR_HEIGHT`.
//
//  This file owns three things:
//   • geometry — where the buttons and the address field are;
//   • routing  — which window points and keystrokes belong to the toolbar,
//                and translating the rest into document coordinates;
//   • paint    — the strip itself.
//  It never touches the `Browser`: it hands `main` a `Command` to carry out.

use browser_engine::{
    browser::Browser,
    css::parser::Color,
    input::KeyEvent,
    layout::{Rect, TextFragment},
    paint::{Canvas, DisplayCommand},
    text::{line_metrics, measure_text},
};

use crate::location_input::{LocationAction, LocationInput};

/// Height of the toolbar strip in pixels.
pub(crate) const TOOLBAR_HEIGHT: usize = 36;

const BUTTON_SIZE: f32 = 28.0;
const BUTTON_GAP: f32 = 4.0;
const MARGIN: f32 = 6.0;
const FIELD_PADDING: f32 = 8.0;
const FONT_SIZE: f32 = 14.0;

const BAR: Color = Color::rgb(241, 243, 244);
const BAR_EDGE: Color = Color::rgb(200, 203, 207);
const BUTTON_HOVER: Color = Color::rgb(222, 225, 229);
const BUTTON_PRESSED: Color = Color::rgb(200, 204, 209);
const ICON: Color = Color::rgb(60, 64, 67);
const ICON_DISABLED: Color = Color::rgb(176, 180, 184);
const FIELD: Color = Color::rgb(255, 255, 255);
const FIELD_EDGE: Color = Color::rgb(196, 199, 203);
const FOCUS_RING: Color = Color::rgb(26, 115, 232);
const ERROR_RING: Color = Color::rgb(217, 48, 37);
const TEXT: Color = Color::rgb(32, 33, 36);
const SELECTION: Color = Color::rgb(179, 215, 255);

/// A clickable part of the toolbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Control {
    Back,
    Forward,
    Reload,
    Address,
}

/// Something the toolbar wants the browser to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    Back,
    Forward,
    Reload,
    /// Open what the user typed into the address field.
    Navigate(String),
}

/// Where a window point lands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum PointerTarget {
    /// Inside the toolbar strip.
    Toolbar,
    /// Inside the page viewport, translated to document coordinates.
    Page { x: f32, y: f32 },
    /// Outside the window.
    Outside,
}

/// What a frame's keystrokes amount to once the toolbar has had its pick.
#[derive(Debug, Default)]
pub(crate) struct KeyDispatch {
    /// Keys for the page. Always empty while the address field is focused.
    pub page: Vec<KeyEvent>,
    pub command: Option<Command>,
}

/// History state the buttons reflect.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Navigation {
    pub can_go_back: bool,
    pub can_go_forward: bool,
}

impl Navigation {
    pub(crate) fn of(browser: &Browser) -> Navigation {
        Navigation {
            can_go_back: browser.can_go_back(),
            can_go_forward: browser.can_go_forward(),
        }
    }

    fn enabled(self, control: Control) -> bool {
        match control {
            Control::Back => self.can_go_back,
            Control::Forward => self.can_go_forward,
            Control::Reload | Control::Address => true,
        }
    }
}

pub(crate) struct Toolbar {
    width: usize,
    /// Present while the address field has keyboard focus.
    editor: Option<LocationInput>,
    /// The last navigation from the field failed; the editor stays open.
    error: Option<String>,
    /// Button under the pointer.
    hovered: Option<Control>,
    /// Button armed by a press; it fires if the release lands on it too.
    pressed: Option<Control>,
    /// A press in the field is dragging out a selection.
    selecting: bool,
    /// Horizontal scroll of the address text, so the caret stays visible.
    text_scroll: f32,
}

impl Toolbar {
    pub(crate) fn new(width: usize) -> Toolbar {
        Toolbar {
            width,
            editor: None,
            error: None,
            hovered: None,
            pressed: None,
            selecting: false,
            text_scroll: 0.0,
        }
    }

    // ── Geometry ──────────────────────────────────────────────────────────

    pub(crate) fn rect(&self, control: Control) -> Rect {
        let top = (TOOLBAR_HEIGHT as f32 - BUTTON_SIZE) / 2.0;
        let button = |slot: f32| Rect {
            x: MARGIN + slot * (BUTTON_SIZE + BUTTON_GAP),
            y: top,
            width: BUTTON_SIZE,
            height: BUTTON_SIZE,
        };
        match control {
            Control::Back => button(0.0),
            Control::Forward => button(1.0),
            Control::Reload => button(2.0),
            Control::Address => {
                let x = MARGIN + 3.0 * (BUTTON_SIZE + BUTTON_GAP) + BUTTON_GAP;
                Rect {
                    x,
                    y: top,
                    width: (self.width as f32 - x - MARGIN).max(0.0),
                    height: BUTTON_SIZE,
                }
            }
        }
    }

    pub(crate) fn control_at(&self, x: f32, y: f32) -> Option<Control> {
        [
            Control::Back,
            Control::Forward,
            Control::Reload,
            Control::Address,
        ]
        .into_iter()
        .find(|&control| contains(self.rect(control), x, y))
    }

    /// Route a window point: toolbar strip, page (in document coordinates,
    /// accounting for the strip above it and the page's scroll), or neither.
    pub(crate) fn route(&self, x: f32, y: f32, page_height: usize, scroll_y: f32) -> PointerTarget {
        let bottom = (TOOLBAR_HEIGHT + page_height) as f32;
        if x < 0.0 || y < 0.0 || x >= self.width as f32 || y >= bottom {
            PointerTarget::Outside
        } else if y < TOOLBAR_HEIGHT as f32 {
            PointerTarget::Toolbar
        } else {
            PointerTarget::Page {
                x,
                y: y - TOOLBAR_HEIGHT as f32 + scroll_y,
            }
        }
    }

    // ── Focus ─────────────────────────────────────────────────────────────

    pub(crate) fn is_editing(&self) -> bool {
        self.editor.is_some()
    }

    /// Ctrl+L: focus the field with everything selected. Already-typed text
    /// is kept, as in other browsers.
    pub(crate) fn focus_address(&mut self, current_url: &str) {
        match &mut self.editor {
            Some(editor) => editor.select_all(),
            None => self.editor = Some(LocationInput::new(current_url.to_string())),
        }
        self.selecting = false;
    }

    pub(crate) fn select_all(&mut self) {
        if let Some(editor) = &mut self.editor {
            editor.select_all();
        }
    }

    /// Leave the field without navigating.
    pub(crate) fn cancel(&mut self) {
        self.editor = None;
        self.error = None;
        self.selecting = false;
        self.text_scroll = 0.0;
    }

    /// The field's navigation succeeded: close it so it shows the live URL.
    pub(crate) fn navigation_succeeded(&mut self) {
        self.cancel();
    }

    /// The field's navigation failed: keep the typed text for correction.
    pub(crate) fn navigation_failed(&mut self, message: String) {
        self.error = Some(message);
    }

    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn editor(&self) -> Option<&LocationInput> {
        self.editor.as_ref()
    }

    // ── Keyboard ──────────────────────────────────────────────────────────

    /// Split a frame's keystrokes between the address field and the page.
    ///
    /// While the field is focused it takes every key, and whatever follows
    /// an Enter or Escape in the same frame is dropped rather than leaking
    /// into the document the user was not typing into.
    pub(crate) fn dispatch_keys(&mut self, events: Vec<KeyEvent>) -> KeyDispatch {
        if self.editor.is_none() {
            return KeyDispatch {
                page: events,
                command: None,
            };
        }
        for event in &events {
            let action = self.editor.as_mut().expect("editing").handle(event);
            match action {
                LocationAction::Continue => self.error = None,
                LocationAction::Cancel => {
                    self.cancel();
                    break;
                }
                LocationAction::Go(target) => {
                    return KeyDispatch {
                        page: Vec::new(),
                        command: Some(Command::Navigate(target)),
                    };
                }
            }
        }
        KeyDispatch::default()
    }

    // ── Pointer ───────────────────────────────────────────────────────────

    /// Track hover, and extend a drag selection while the button is held.
    pub(crate) fn pointer_moved(&mut self, x: f32, y: f32) {
        self.hovered = self.control_at(x, y);
        if self.selecting {
            let index = self.char_index_at(x);
            if let Some(editor) = &mut self.editor {
                editor.place_caret(index, true);
            }
        }
    }

    pub(crate) fn pointer_left(&mut self) {
        self.hovered = None;
    }

    /// The pointer is over the address field (for an I-beam cursor).
    pub(crate) fn over_address(&self) -> bool {
        self.hovered == Some(Control::Address)
    }

    /// A press inside the toolbar strip.
    pub(crate) fn press(&mut self, x: f32, y: f32, current_url: &str, shift: bool) {
        match self.control_at(x, y) {
            Some(Control::Address) => match &mut self.editor {
                // First click focuses and selects everything, like Ctrl+L.
                None => self.focus_address(current_url),
                Some(_) => {
                    let index = self.char_index_at(x);
                    let editor = self.editor.as_mut().expect("editing");
                    editor.place_caret(index, shift);
                    self.selecting = true;
                }
            },
            Some(button) => {
                // Pressing a button takes focus away from the field.
                self.cancel();
                self.pressed = Some(button);
            }
            None => {}
        }
    }

    /// The button came up. A button fires only if the press started on it
    /// and the release lands on it, and only when it is enabled.
    pub(crate) fn release(&mut self, x: f32, y: f32, navigation: Navigation) -> Option<Command> {
        self.selecting = false;
        let armed = self.pressed.take()?;
        if self.control_at(x, y) != Some(armed) || !navigation.enabled(armed) {
            return None;
        }
        match armed {
            Control::Back => Some(Command::Back),
            Control::Forward => Some(Command::Forward),
            Control::Reload => Some(Command::Reload),
            Control::Address => None,
        }
    }

    /// The char boundary nearest to window x inside the address field.
    fn char_index_at(&self, x: f32) -> usize {
        let Some(editor) = &self.editor else {
            return 0;
        };
        let target = x - self.text_origin();
        let mut advance = 0.0;
        for (index, character) in editor.value().chars().enumerate() {
            let width = measure_text(character.encode_utf8(&mut [0; 4]), FONT_SIZE);
            if target < advance + width / 2.0 {
                return index;
            }
            advance += width;
        }
        editor.value().chars().count()
    }

    fn text_origin(&self) -> f32 {
        self.rect(Control::Address).x + FIELD_PADDING - self.text_scroll
    }

    /// Scroll the field's text so the caret is inside it.
    fn scroll_caret_into_view(&mut self) {
        let Some(editor) = &self.editor else {
            self.text_scroll = 0.0;
            return;
        };
        let visible = (self.rect(Control::Address).width - 2.0 * FIELD_PADDING).max(1.0);
        let text_width = measure_text(editor.value(), FONT_SIZE);
        let caret_x = prefix_width(editor.value(), editor.caret());
        if caret_x < self.text_scroll {
            self.text_scroll = caret_x;
        } else if caret_x > self.text_scroll + visible {
            self.text_scroll = caret_x - visible;
        }
        self.text_scroll = self.text_scroll.clamp(0.0, (text_width - visible).max(0.0));
    }

    // ── Paint ─────────────────────────────────────────────────────────────

    /// Draw the strip for a page at `address`.
    pub(crate) fn paint(&mut self, address: &str, navigation: Navigation) -> Canvas {
        self.scroll_caret_into_view();
        let mut canvas = Canvas::new(self.width, TOOLBAR_HEIGHT, BAR);
        canvas.paint(&DisplayCommand::SolidColor(
            BAR_EDGE,
            Rect {
                x: 0.0,
                y: TOOLBAR_HEIGHT as f32 - 1.0,
                width: self.width as f32,
                height: 1.0,
            },
        ));

        for control in [Control::Back, Control::Forward, Control::Reload] {
            self.paint_button(&mut canvas, control, navigation.enabled(control));
        }
        self.paint_address(&mut canvas, address);
        canvas
    }

    fn paint_button(&self, canvas: &mut Canvas, control: Control, enabled: bool) {
        let rect = self.rect(control);
        let fill = if !enabled {
            None
        } else if self.pressed == Some(control) && self.hovered == Some(control) {
            Some(BUTTON_PRESSED)
        } else if self.hovered == Some(control) || self.pressed == Some(control) {
            Some(BUTTON_HOVER)
        } else {
            None
        };
        if let Some(fill) = fill {
            canvas.paint(&DisplayCommand::RoundedRect(fill, rect, 6.0));
        }
        let color = if enabled { ICON } else { ICON_DISABLED };
        let (cx, cy) = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
        match control {
            Control::Back => {
                let shape = arrow(cx, cy, -1.0);
                stroke(canvas, rect, color, |x, y| distance_to(&shape, x, y));
            }
            Control::Forward => {
                let shape = arrow(cx, cy, 1.0);
                stroke(canvas, rect, color, |x, y| distance_to(&shape, x, y));
            }
            Control::Reload => stroke(canvas, rect, color, |x, y| reload_distance(cx, cy, x, y)),
            Control::Address => {}
        }
    }

    fn paint_address(&self, canvas: &mut Canvas, address: &str) {
        let field = self.rect(Control::Address);
        if field.width <= 2.0 * FIELD_PADDING {
            return;
        }
        let ring = match (&self.editor, &self.error) {
            (Some(_), Some(_)) => Some(ERROR_RING),
            (Some(_), None) => Some(FOCUS_RING),
            (None, _) => None,
        };
        // A border is a slightly larger rounded rect behind the white one.
        let (edge, thickness) = match ring {
            Some(color) => (color, 2.0),
            None => (FIELD_EDGE, 1.0),
        };
        canvas.paint(&DisplayCommand::RoundedRect(edge, field, 8.0));
        canvas.paint(&DisplayCommand::RoundedRect(
            FIELD,
            inset(field, thickness),
            8.0 - thickness,
        ));

        let text = self
            .editor
            .as_ref()
            .map_or(address, |editor| editor.value());
        let origin = self.text_origin();
        let metrics = line_metrics(FONT_SIZE);
        let baseline =
            (field.y + (field.height - metrics.new_line_size) / 2.0 + metrics.ascent).round();
        let line_top = baseline - metrics.ascent;

        canvas.paint(&DisplayCommand::PushClip(Rect {
            x: field.x + FIELD_PADDING - 1.0,
            y: field.y + thickness,
            width: field.width - 2.0 * FIELD_PADDING + 2.0,
            height: field.height - 2.0 * thickness,
        }));
        if let Some(editor) = &self.editor {
            if let Some((start, end)) = editor.selection() {
                let x0 = origin + prefix_width(text, start);
                let x1 = origin + prefix_width(text, end);
                canvas.paint(&DisplayCommand::SolidColor(
                    SELECTION,
                    Rect {
                        x: x0,
                        y: line_top,
                        width: x1 - x0,
                        height: metrics.new_line_size,
                    },
                ));
            }
        }
        canvas.paint(&DisplayCommand::Text(TextFragment {
            text: text.to_string(),
            rect: Rect {
                x: origin,
                y: line_top,
                width: measure_text(text, FONT_SIZE),
                height: metrics.new_line_size,
            },
            baseline,
            color: TEXT,
            font_size: FONT_SIZE,
            underline: false,
            strikethrough: false,
            text_shadow: None,
        }));
        if let Some(editor) = &self.editor {
            if editor.selection().is_none() {
                let caret = (origin + prefix_width(text, editor.caret())).round();
                canvas.paint(&DisplayCommand::SolidColor(
                    TEXT,
                    Rect {
                        x: caret,
                        y: line_top,
                        width: 1.0,
                        height: metrics.new_line_size,
                    },
                ));
            }
        }
        canvas.paint(&DisplayCommand::PopClip);
    }
}

/// Stack the toolbar strip on top of the page into one window buffer.
pub(crate) fn compose(toolbar: &Canvas, page: &Canvas) -> Vec<u32> {
    let mut frame = toolbar.to_u32_buffer();
    frame.extend(page.to_u32_buffer());
    frame
}

fn contains(rect: Rect, x: f32, y: f32) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

fn inset(rect: Rect, by: f32) -> Rect {
    Rect {
        x: rect.x + by,
        y: rect.y + by,
        width: (rect.width - 2.0 * by).max(0.0),
        height: (rect.height - 2.0 * by).max(0.0),
    }
}

fn prefix_width(text: &str, chars: usize) -> f32 {
    let end = text
        .char_indices()
        .nth(chars)
        .map_or(text.len(), |(i, _)| i);
    measure_text(&text[..end], FONT_SIZE)
}

// ── Icons ─────────────────────────────────────────────────────────────────────
//
// Icons are strokes described by a distance field and rasterised with a
// one-pixel soft edge, so they need no icon font and look the same anywhere.

const ICON_STROKE: f32 = 2.0;

#[derive(Debug, Clone, Copy)]
struct Segment {
    from: (f32, f32),
    to: (f32, f32),
}

impl Segment {
    fn distance(&self, x: f32, y: f32) -> f32 {
        let (dx, dy) = (self.to.0 - self.from.0, self.to.1 - self.from.1);
        let length = dx * dx + dy * dy;
        let t = if length == 0.0 {
            0.0
        } else {
            (((x - self.from.0) * dx + (y - self.from.1) * dy) / length).clamp(0.0, 1.0)
        };
        let (px, py) = (self.from.0 + t * dx, self.from.1 + t * dy);
        ((x - px).powi(2) + (y - py).powi(2)).sqrt()
    }
}

/// A horizontal arrow pointing left (`direction` -1) or right (+1).
fn arrow(cx: f32, cy: f32, direction: f32) -> Vec<Segment> {
    let tip = (cx + 6.0 * direction, cy);
    vec![
        Segment {
            from: (cx - 6.0 * direction, cy),
            to: tip,
        },
        Segment {
            from: tip,
            to: (tip.0 - 5.0 * direction, cy - 5.0),
        },
        Segment {
            from: tip,
            to: (tip.0 - 5.0 * direction, cy + 5.0),
        },
    ]
}

/// A circular arrow: a ring open at the upper right, ending at the top in
/// an arrowhead that points clockwise across the gap.
fn reload_distance(cx: f32, cy: f32, x: f32, y: f32) -> f32 {
    const RADIUS: f32 = 6.0;
    let (dx, dy) = (x - cx, y - cy);
    // Screen y grows downwards, so the top of the ring is at -π/2.
    let gap = (-std::f32::consts::FRAC_PI_2..-0.4).contains(&dy.atan2(dx));
    let ring = if gap {
        f32::INFINITY
    } else {
        ((dx * dx + dy * dy).sqrt() - RADIUS).abs()
    };
    let tip = (cx + 2.5, cy - RADIUS);
    let head = [
        Segment {
            from: (cx, cy - RADIUS),
            to: tip,
        },
        Segment {
            from: tip,
            to: (tip.0 - 3.5, tip.1 - 3.5),
        },
        Segment {
            from: tip,
            to: (tip.0 - 3.5, tip.1 + 3.5),
        },
    ];
    ring.min(distance_to(&head, x, y))
}

fn distance_to(segments: &[Segment], x: f32, y: f32) -> f32 {
    segments
        .iter()
        .map(|segment| segment.distance(x, y))
        .fold(f32::INFINITY, f32::min)
}

/// Paint every pixel of `rect` whose distance to the shape is within half
/// the stroke width, with an anti-aliased edge.
fn stroke(canvas: &mut Canvas, rect: Rect, color: Color, distance: impl Fn(f32, f32) -> f32) {
    let (x0, y0) = (rect.x.floor() as i32, rect.y.floor() as i32);
    let (x1, y1) = (
        (rect.x + rect.width).ceil() as i32,
        (rect.y + rect.height).ceil() as i32,
    );
    for y in y0..y1 {
        for x in x0..x1 {
            let d = distance(x as f32 + 0.5, y as f32 + 0.5);
            let coverage = (ICON_STROKE / 2.0 + 0.5 - d).clamp(0.0, 1.0);
            if coverage > 0.0 {
                canvas.paint(&DisplayCommand::SolidColor(
                    Color::rgba(color.r, color.g, color.b, (coverage * 255.0) as u8),
                    Rect {
                        x: x as f32,
                        y: y as f32,
                        width: 1.0,
                        height: 1.0,
                    },
                ));
            }
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use browser_engine::{
        input::{Key, Modifiers},
        net::{LoadError, MemoryLoader, Resource, ResourceLoader, Url},
        script::dom_api,
    };

    const WIDTH: usize = 640;
    const PAGE_HEIGHT: usize = 400;
    const BOTH: Navigation = Navigation {
        can_go_back: true,
        can_go_forward: true,
    };

    fn key(key: Key) -> KeyEvent {
        KeyEvent::with_modifiers(key, Modifiers::NONE)
    }

    fn chars(text: &str) -> Vec<KeyEvent> {
        text.chars().map(|c| key(Key::Character(c))).collect()
    }

    fn center(rect: Rect) -> (f32, f32) {
        (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0)
    }

    fn click(toolbar: &mut Toolbar, control: Control, url: &str) -> Option<Command> {
        let (x, y) = center(toolbar.rect(control));
        toolbar.press(x, y, url, false);
        toolbar.release(x, y, BOTH)
    }

    fn pixel(canvas: &Canvas, x: f32, y: f32) -> Color {
        let i = (y as usize * canvas.width + x as usize) * 3;
        Color::rgb(canvas.pixels[i], canvas.pixels[i + 1], canvas.pixels[i + 2])
    }

    // ── Geometry and pointer routing ──────────────────────────────────────

    #[test]
    fn controls_are_laid_out_left_to_right_inside_the_strip() {
        let toolbar = Toolbar::new(WIDTH);
        let rects = [
            Control::Back,
            Control::Forward,
            Control::Reload,
            Control::Address,
        ]
        .map(|control| toolbar.rect(control));
        for pair in rects.windows(2) {
            assert!(pair[0].x + pair[0].width <= pair[1].x, "{pair:?} overlap");
        }
        for rect in rects {
            assert!(rect.y >= 0.0 && rect.y + rect.height <= TOOLBAR_HEIGHT as f32);
        }
        let address = rects[3];
        assert!(address.x + address.width <= WIDTH as f32);
        assert!(
            address.width > WIDTH as f32 / 2.0,
            "the field takes the rest of the row"
        );
        for control in [
            Control::Back,
            Control::Forward,
            Control::Reload,
            Control::Address,
        ] {
            let (x, y) = center(toolbar.rect(control));
            assert_eq!(toolbar.control_at(x, y), Some(control));
        }
        assert_eq!(toolbar.control_at(1.0, 1.0), None);
    }

    #[test]
    fn window_points_route_to_toolbar_page_or_nowhere() {
        let toolbar = Toolbar::new(WIDTH);
        let route = |x, y, scroll| toolbar.route(x, y, PAGE_HEIGHT, scroll);
        assert_eq!(route(10.0, 0.0, 0.0), PointerTarget::Toolbar);
        assert_eq!(route(10.0, 35.9, 0.0), PointerTarget::Toolbar);
        // The first page row is directly below the strip.
        assert_eq!(
            route(10.0, TOOLBAR_HEIGHT as f32, 0.0),
            PointerTarget::Page { x: 10.0, y: 0.0 }
        );
        // Document coordinates include the page's scroll offset.
        assert_eq!(
            route(25.0, 136.0, 250.0),
            PointerTarget::Page { x: 25.0, y: 350.0 }
        );
        let bottom = (TOOLBAR_HEIGHT + PAGE_HEIGHT) as f32;
        assert_eq!(route(10.0, bottom, 0.0), PointerTarget::Outside);
        assert_eq!(route(-1.0, 50.0, 0.0), PointerTarget::Outside);
        assert_eq!(route(WIDTH as f32, 50.0, 0.0), PointerTarget::Outside);
        assert_eq!(route(10.0, -3.0, 0.0), PointerTarget::Outside);
    }

    // ── Keyboard routing ──────────────────────────────────────────────────

    #[test]
    fn keys_go_to_the_page_only_while_the_field_is_unfocused() {
        let mut toolbar = Toolbar::new(WIDTH);
        let dispatch = toolbar.dispatch_keys(chars("ab"));
        assert_eq!(dispatch.page.len(), 2);
        assert_eq!(dispatch.command, None);

        toolbar.focus_address("http://old.test/");
        let dispatch = toolbar.dispatch_keys(chars("new.test"));
        assert!(dispatch.page.is_empty(), "typing must not reach the DOM");
        assert_eq!(dispatch.command, None);
        assert_eq!(toolbar.editor().unwrap().value(), "new.test");
    }

    #[test]
    fn enter_navigates_and_drops_the_rest_of_the_frame() {
        let mut toolbar = Toolbar::new(WIDTH);
        toolbar.focus_address("http://old.test/");
        let mut events = chars("https://next.test/");
        events.push(key(Key::Enter));
        events.extend(chars("zz"));
        let dispatch = toolbar.dispatch_keys(events);
        assert_eq!(
            dispatch.command,
            Some(Command::Navigate("https://next.test/".into()))
        );
        assert!(dispatch.page.is_empty());
        // The field stays focused until the navigation reports back.
        assert!(toolbar.is_editing());
    }

    #[test]
    fn escape_cancels_without_leaking_later_keys_to_the_page() {
        let mut toolbar = Toolbar::new(WIDTH);
        toolbar.focus_address("http://old.test/");
        let mut events = chars("typo");
        events.push(key(Key::Escape));
        events.extend(chars("xy"));
        let dispatch = toolbar.dispatch_keys(events);
        assert!(dispatch.page.is_empty() && dispatch.command.is_none());
        assert!(!toolbar.is_editing());
        // Escape again (next frame) is the window's own close shortcut, not
        // the toolbar's: nothing is left to cancel.
        assert_eq!(toolbar.dispatch_keys(chars("q")).page.len(), 1);
    }

    #[test]
    fn ctrl_l_selects_all_without_discarding_typed_text() {
        let mut toolbar = Toolbar::new(WIDTH);
        toolbar.focus_address("http://old.test/");
        toolbar.dispatch_keys(chars("draft"));
        toolbar.focus_address("http://old.test/");
        let editor = toolbar.editor().unwrap();
        assert_eq!(editor.value(), "draft");
        assert_eq!(editor.selection(), Some((0, 5)));
    }

    #[test]
    fn failed_navigation_keeps_the_field_open_until_corrected() {
        let mut toolbar = Toolbar::new(WIDTH);
        toolbar.focus_address("http://old.test/");
        toolbar.dispatch_keys(chars("nope://x"));
        toolbar.navigation_failed("Could not open nope://x".into());
        assert!(toolbar.is_editing());
        assert_eq!(toolbar.editor().unwrap().value(), "nope://x");
        assert!(toolbar.error().is_some());
        // Editing again clears the error.
        toolbar.dispatch_keys(vec![key(Key::Backspace)]);
        assert!(toolbar.error().is_none());

        toolbar.navigation_succeeded();
        assert!(!toolbar.is_editing());
    }

    // ── Pointer ───────────────────────────────────────────────────────────

    #[test]
    fn clicking_the_field_focuses_it_then_places_and_drags_the_caret() {
        let mut toolbar = Toolbar::new(WIDTH);
        let field = toolbar.rect(Control::Address);
        let (x, y) = center(field);
        toolbar.press(x, y, "http://site.test/", false);
        assert_eq!(toolbar.release(x, y, BOTH), None);
        let editor = toolbar.editor().expect("click focuses the field");
        assert_eq!(editor.selection(), Some((0, 17)));

        // A second click at the start of the text collapses to a caret there.
        let start = field.x + FIELD_PADDING;
        toolbar.press(start, y, "http://site.test/", false);
        assert_eq!(toolbar.editor().unwrap().caret(), 0);
        assert_eq!(toolbar.editor().unwrap().selection(), None);
        // Dragging to the far right selects the whole value.
        toolbar.pointer_moved(field.x + field.width + 50.0, y);
        assert_eq!(toolbar.editor().unwrap().selection(), Some((0, 17)));
        toolbar.release(field.x + field.width + 50.0, y, BOTH);
        // Moving after release no longer changes the selection.
        toolbar.pointer_moved(start, y);
        assert_eq!(toolbar.editor().unwrap().selection(), Some((0, 17)));
    }

    #[test]
    fn caret_lands_on_the_nearest_character_boundary() {
        let mut toolbar = Toolbar::new(WIDTH);
        toolbar.focus_address("abcdef");
        let y = center(toolbar.rect(Control::Address)).1;
        let origin = toolbar.text_origin();
        let three = prefix_width("abcdef", 3);
        toolbar.press(origin + three + 0.5, y, "", false);
        assert_eq!(toolbar.editor().unwrap().caret(), 3);
        toolbar.press(origin + three, y, "", true);
        toolbar.press(origin + prefix_width("abcdef", 5) - 0.5, y, "", true);
        assert_eq!(toolbar.editor().unwrap().selection(), Some((3, 5)));
    }

    #[test]
    fn buttons_fire_on_release_over_the_same_enabled_button() {
        let mut toolbar = Toolbar::new(WIDTH);
        assert_eq!(click(&mut toolbar, Control::Back, ""), Some(Command::Back));
        assert_eq!(
            click(&mut toolbar, Control::Forward, ""),
            Some(Command::Forward)
        );
        assert_eq!(
            click(&mut toolbar, Control::Reload, ""),
            Some(Command::Reload)
        );

        // Released elsewhere: cancelled.
        let (x, y) = center(toolbar.rect(Control::Back));
        toolbar.press(x, y, "", false);
        let (fx, fy) = center(toolbar.rect(Control::Forward));
        assert_eq!(toolbar.release(fx, fy, BOTH), None);

        // Disabled: no command.
        toolbar.press(x, y, "", false);
        assert_eq!(toolbar.release(x, y, Navigation::default()), None);

        // A release with no press is nothing.
        assert_eq!(toolbar.release(x, y, BOTH), None);
    }

    #[test]
    fn pressing_a_button_takes_focus_from_the_field() {
        let mut toolbar = Toolbar::new(WIDTH);
        toolbar.focus_address("http://old.test/");
        assert_eq!(
            click(&mut toolbar, Control::Reload, ""),
            Some(Command::Reload)
        );
        assert!(!toolbar.is_editing());
    }

    // ── Paint ─────────────────────────────────────────────────────────────

    #[test]
    fn strip_and_page_compose_into_one_window_buffer() {
        let mut toolbar = Toolbar::new(WIDTH);
        let strip = toolbar.paint("http://site.test/", BOTH);
        assert_eq!((strip.width, strip.height), (WIDTH, TOOLBAR_HEIGHT));
        let page = Canvas::new(WIDTH, PAGE_HEIGHT, Color::rgb(1, 2, 3));
        let frame = compose(&strip, &page);
        assert_eq!(frame.len(), WIDTH * (TOOLBAR_HEIGHT + PAGE_HEIGHT));
        assert_eq!(frame[0], 0xF1F3F4, "top-left is toolbar background");
        assert_eq!(frame[WIDTH * TOOLBAR_HEIGHT], 0x010203, "first page row");
        assert_eq!(*frame.last().unwrap(), 0x010203);
    }

    #[test]
    fn field_border_shows_focus_and_error_state() {
        let mut toolbar = Toolbar::new(WIDTH);
        let field = toolbar.rect(Control::Address);
        let (x, y) = (field.x + field.width / 2.0, field.y + 0.5);
        assert_eq!(pixel(&toolbar.paint("a", BOTH), x, y), FIELD_EDGE);
        toolbar.focus_address("a");
        assert_eq!(pixel(&toolbar.paint("a", BOTH), x, y), FOCUS_RING);
        toolbar.navigation_failed("nope".into());
        assert_eq!(pixel(&toolbar.paint("a", BOTH), x, y), ERROR_RING);
    }

    #[test]
    fn disabled_history_buttons_are_drawn_lighter() {
        let mut toolbar = Toolbar::new(WIDTH);
        let layout = Toolbar::new(WIDTH);
        let darkest = |canvas: &Canvas, control| {
            let rect = layout.rect(control);
            let mut min = 255u8;
            for y in rect.y as usize..(rect.y + rect.height) as usize {
                for x in rect.x as usize..(rect.x + rect.width) as usize {
                    min = min.min(pixel(canvas, x as f32, y as f32).r);
                }
            }
            min
        };
        let enabled = toolbar.paint("a", BOTH);
        let disabled = toolbar.paint("a", Navigation::default());
        assert_eq!(darkest(&enabled, Control::Back), ICON.r);
        assert_eq!(darkest(&disabled, Control::Back), ICON_DISABLED.r);
        assert_eq!(darkest(&disabled, Control::Reload), ICON.r);
    }

    #[test]
    fn long_urls_are_clipped_to_the_field_and_scroll_with_the_caret() {
        let mut toolbar = Toolbar::new(320);
        let long = format!("http://site.test/{}", "segment/".repeat(60));
        let field = toolbar.rect(Control::Address);
        let canvas = toolbar.paint(&long, BOTH);
        // Nothing is drawn over the right margin of the strip.
        for y in 0..TOOLBAR_HEIGHT - 1 {
            for x in (field.x + field.width) as usize + 1..320 {
                assert_eq!(pixel(&canvas, x as f32, y as f32), BAR, "({x},{y})");
            }
        }
        assert_eq!(toolbar.text_scroll, 0.0, "unfocused shows the start");

        toolbar.focus_address(&long);
        toolbar.dispatch_keys(vec![key(Key::End)]);
        toolbar.paint(&long, BOTH);
        let caret = toolbar.text_origin() + prefix_width(&long, long.chars().count());
        assert!(toolbar.text_scroll > 0.0);
        assert!(caret <= field.x + field.width - FIELD_PADDING + 0.5);
        assert!(caret >= field.x + FIELD_PADDING);

        toolbar.dispatch_keys(vec![key(Key::Home)]);
        toolbar.paint(&long, BOTH);
        assert_eq!(toolbar.text_scroll, 0.0);
    }

    #[test]
    fn address_text_is_drawn_when_a_font_is_available() {
        if browser_engine::text::rasterize('h', FONT_SIZE).is_none() {
            return; // headless machine without a system font
        }
        let mut toolbar = Toolbar::new(WIDTH);
        let field = toolbar.rect(Control::Address);
        let dark_pixels = |canvas: &Canvas| {
            let mut count = 0;
            for y in field.y as usize + 3..(field.y + field.height) as usize - 3 {
                for x in field.x as usize + 3..(field.x + field.width) as usize - 3 {
                    if pixel(canvas, x as f32, y as f32).r < 128 {
                        count += 1;
                    }
                }
            }
            count
        };
        assert_eq!(dark_pixels(&toolbar.paint("", BOTH)), 0);
        assert!(dark_pixels(&toolbar.paint("http://site.test/", BOTH)) > 30);
    }

    // ── Against a real Browser ────────────────────────────────────────────

    fn browser(loader: impl ResourceLoader + 'static, url: &str) -> Browser {
        Browser::open(Box::new(loader), &Url::parse(url).unwrap()).unwrap()
    }

    #[test]
    fn typing_in_the_field_never_reaches_page_key_listeners() {
        let mut site = MemoryLoader::new();
        site.insert(
            "demo:///index.html",
            r#"<html><body><p id="log">none</p>
               <script>
                 let keys = 0;
                 document.body.addEventListener("keydown", function () {
                   keys++;
                   document.getElementById("log").textContent = "keys " + keys;
                 });
               </script></body></html>"#,
        );
        let mut browser = browser(site, "demo:///index.html");
        let mut toolbar = Toolbar::new(WIDTH);

        // Same wiring as the window loop: the page only gets what the
        // toolbar hands back.
        let deliver = |toolbar: &mut Toolbar, browser: &mut Browser, events| {
            let dispatch = toolbar.dispatch_keys(events);
            for event in dispatch.page {
                browser.press_key(&event);
            }
            dispatch.command
        };

        toolbar.focus_address(&browser.url().to_string());
        let mut events = chars("secret");
        events.push(key(Key::Escape));
        assert_eq!(deliver(&mut toolbar, &mut browser, events), None);
        let log = |browser: &Browser| dom_api::outer_html(&browser.document().dom);
        assert!(
            log(&browser).contains(">none<"),
            "no keydown reached the page"
        );

        deliver(&mut toolbar, &mut browser, chars("ok"));
        assert!(log(&browser).contains(">keys 2<"));
    }

    #[test]
    fn history_state_drives_the_buttons() {
        let mut site = MemoryLoader::new();
        site.insert("demo:///a.html", "<p>a</p>");
        site.insert("demo:///b.html", "<p>b</p>");
        let mut browser = browser(site, "demo:///a.html");
        let nav = Navigation::of(&browser);
        assert!(!nav.can_go_back && !nav.can_go_forward);
        browser
            .navigate(&Url::parse("demo:///b.html#part").unwrap())
            .unwrap();
        assert_eq!(browser.url().to_string(), "demo:///b.html#part");
        assert!(Navigation::of(&browser).can_go_back);
        assert!(browser.back());
        let nav = Navigation::of(&browser);
        assert!(!nav.can_go_back && nav.can_go_forward);
    }

    /// Answers every request with the same page, as though the server had
    /// redirected to `demo:///final.html`.
    struct Redirecting;

    impl ResourceLoader for Redirecting {
        fn load(&self, _url: &Url) -> Result<Resource, LoadError> {
            Ok(Resource::new(
                Url::parse("demo:///final.html").unwrap(),
                Some("text/html".into()),
                b"<p>final</p>".to_vec(),
            ))
        }
    }

    /// The field shows `Browser::url()` once a navigation commits, so that
    /// must be where the load ended up rather than what was typed.
    #[test]
    fn committed_url_is_the_redirect_destination() {
        let mut browser = browser(Redirecting, "demo:///start.html");
        assert_eq!(browser.url().to_string(), "demo:///final.html");
        browser
            .navigate(&Url::parse("demo:///moved.html").unwrap())
            .unwrap();
        assert_eq!(browser.url().to_string(), "demo:///final.html");
    }
}
