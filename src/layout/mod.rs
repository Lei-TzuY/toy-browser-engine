// ============================================================
//  layout/mod.rs  —  Block layout engine
// ============================================================
//
//  Builds a box tree from the styled tree and computes each
//  box's position and size.
//
//  New in this revision:
//   • text-align: left / center / right (via inherited CSS property)
//   • margin: auto — distributes free horizontal space
//   • min-width / max-width clamping
//   • position: relative with top/right/bottom/left offsets

use std::rc::Rc;

use crate::css::parser::{CalcExpr, Color, Unit, Value};
use crate::dom::{ElementData, ElementId, NodeType};
use crate::image::RasterImage;
use crate::style::{Display, Position, StyledNode};
use crate::text::{line_metrics, measure_text};

// ── Geometry primitives ───────────────────────────────────────────────────────

#[derive(Debug, Default, Clone, Copy)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn expanded_by(self, e: EdgeSizes) -> Self {
        Self {
            x: self.x - e.left,
            y: self.y - e.top,
            width: self.width + e.left + e.right,
            height: self.height + e.top + e.bottom,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct EdgeSizes {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Dimensions {
    pub content: Rect,
    pub padding: EdgeSizes,
    pub border: EdgeSizes,
    pub margin: EdgeSizes,
}

impl Dimensions {
    pub fn padding_box(self) -> Rect {
        self.content.expanded_by(self.padding)
    }
    pub fn border_box(self) -> Rect {
        self.padding_box().expanded_by(self.border)
    }
    pub fn margin_box(self) -> Rect {
        self.border_box().expanded_by(self.margin)
    }
}

// ── Text alignment ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

// ── Box tree ──────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum BoxType<'a> {
    Block(&'a StyledNode<'a>),
    Flex(&'a StyledNode<'a>),
    Grid(&'a StyledNode<'a>),
    Table(&'a StyledNode<'a>),
    TableRow(&'a StyledNode<'a>),
    TableCell(&'a StyledNode<'a>),
    Inline(&'a StyledNode<'a>),
    /// Inline element with its own block-formatting context.
    InlineBlock(&'a StyledNode<'a>),
    /// Wrapper for a run of inline children inside a block container.
    AnonymousBlock,
}

#[derive(Debug, Clone)]
pub struct TextFragment {
    pub text: String,
    pub rect: Rect,
    pub baseline: f32,
    pub color: Color,
    pub font_size: f32,
    pub underline: bool,
    pub strikethrough: bool,
}

#[derive(Debug, Clone)]
pub struct LineBox {
    pub rect: Rect,
    pub baseline: f32,
    pub fragments: Vec<TextFragment>,
    /// Positioned inline-block boxes: (child_idx_in_parent, margin-box x, margin-box y).
    pub inline_boxes: Vec<(usize, f32, f32)>,
}

#[derive(Debug)]
pub struct LayoutBox<'a> {
    pub dimensions: Dimensions,
    pub box_type: BoxType<'a>,
    pub children: Vec<LayoutBox<'a>>,
    pub line_boxes: Vec<LineBox>,
    text_style: TextStyle,
    text_align: TextAlign,
    /// Viewport width, used as the containing block for `position: fixed` children.
    viewport_w: f32,
    /// Decoded bitmap for a replaced element (`<img>`), when one loaded.
    image: Option<Rc<RasterImage>>,
    /// True when this box is the document's focused element.
    focused: bool,
    /// Height chosen by replaced-element sizing, applied in `calc_height`.
    replaced_height: Option<f32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum TextTransform {
    #[default]
    None,
    Uppercase,
    Lowercase,
    Capitalize,
}

#[derive(Debug, Clone, Copy)]
struct TextStyle {
    color: Color,
    font_size: f32,
    line_height: f32,
    no_wrap: bool,
    underline: bool,
    strikethrough: bool,
    text_transform: TextTransform,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            color: Color::rgb(0, 0, 0),
            font_size: 16.0,
            line_height: 1.0,
            no_wrap: false,
            underline: false,
            strikethrough: false,
            text_transform: TextTransform::None,
        }
    }
}

#[derive(Debug)]
struct InlinePiece {
    text: String,
    style: TextStyle,
    no_wrap: bool,
    /// Set for inline-block pieces, which sit on the line as opaque boxes.
    inline_box: Option<InlineBoxPiece>,
}

/// An inline-level box waiting to be placed on a line.
#[derive(Debug, Clone, Copy)]
struct InlineBoxPiece {
    /// Index of the box among its parent's children.
    index: usize,
    /// Margin-box size.
    width: f32,
    height: f32,
    /// Distance from the top of the margin box down to the baseline it aligns on.
    baseline: f32,
}

impl<'a> LayoutBox<'a> {
    fn new(box_type: BoxType<'a>, text_style: TextStyle, text_align: TextAlign) -> Self {
        Self {
            box_type,
            dimensions: Default::default(),
            children: Vec::new(),
            line_boxes: Vec::new(),
            text_style,
            text_align,
            viewport_w: 0.0,
            image: None,
            focused: false,
            replaced_height: None,
        }
    }

    /// The decoded image of a replaced element, if it loaded.
    pub fn image(&self) -> Option<&Rc<RasterImage>> {
        self.image.as_ref()
    }

    /// True when this box renders the focused element.
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// The styled node this box was generated from, if it has one
    /// (anonymous boxes do not).
    pub fn styled_node(&self) -> Option<&StyledNode<'a>> {
        match &self.box_type {
            BoxType::Block(s)
            | BoxType::Flex(s)
            | BoxType::Grid(s)
            | BoxType::Table(s)
            | BoxType::TableRow(s)
            | BoxType::TableCell(s)
            | BoxType::Inline(s)
            | BoxType::InlineBlock(s) => Some(s),
            BoxType::AnonymousBlock => None,
        }
    }

    fn style(&self) -> Option<&StyledNode<'a>> {
        self.styled_node()
    }

    pub fn hit_test(&self, x: f32, y: f32) -> Option<&'a crate::dom::Node> {
        let bb = self.dimensions.border_box();
        if x >= bb.x && x <= bb.x + bb.width && y >= bb.y && y <= bb.y + bb.height {
            for child in self.children.iter().rev() {
                if let Some(n) = child.hit_test(x, y) {
                    return Some(n);
                }
            }
            if let Some(s) = self.style() {
                return Some(s.node);
            }
        }
        None
    }

    /// Move this box and everything inside it by `(dx, dy)`.
    ///
    /// Inline-blocks are laid out before their final position on the line is
    /// known, so the whole subtree — including line boxes and text fragments,
    /// which hold absolute coordinates — has to be shifted afterwards.
    fn translate(&mut self, dx: f32, dy: f32) {
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        self.dimensions.content.x += dx;
        self.dimensions.content.y += dy;

        for line in &mut self.line_boxes {
            line.rect.x += dx;
            line.rect.y += dy;
            line.baseline += dy;
            for fragment in &mut line.fragments {
                fragment.rect.x += dx;
                fragment.rect.y += dy;
                fragment.baseline += dy;
            }
            for (_, box_x, box_y) in &mut line.inline_boxes {
                *box_x += dx;
                *box_y += dy;
            }
        }
        for child in &mut self.children {
            child.translate(dx, dy);
        }
    }

    /// Distance from the top of this box's margin box to the baseline it sits
    /// on when placed inline.
    ///
    /// CSS aligns an inline-block on the baseline of its last line box; a box
    /// with no text of its own — an image, say — aligns on its bottom margin
    /// edge instead.
    fn inline_baseline(&self) -> f32 {
        let d = self.dimensions;
        match self.last_line_baseline() {
            Some(offset) => d.margin.top + d.border.top + d.padding.top + offset,
            None => d.margin_box().height,
        }
    }

    /// Baseline of the last line box inside this subtree, relative to this
    /// box's content top.
    fn last_line_baseline(&self) -> Option<f32> {
        if let Some(line) = self.line_boxes.last() {
            return Some(line.baseline - self.dimensions.content.y);
        }
        for child in self.children.iter().rev() {
            if let Some(offset) = child.last_line_baseline() {
                // Convert from the child's content origin into ours.
                return Some(offset + child.dimensions.content.y - self.dimensions.content.y);
            }
        }
        None
    }

    /// Move an already-laid-out box so its margin box starts at `(x, y)`.
    /// Used by inline layout and flex, which both size a box before they know
    /// where it goes.
    fn place_margin_box_at(&mut self, x: f32, y: f32) {
        let d = self.dimensions;
        let target_x = x + d.margin.left + d.border.left + d.padding.left;
        let target_y = y + d.margin.top + d.border.top + d.padding.top;
        self.translate(target_x - d.content.x, target_y - d.content.y);
    }

    /// Return the inline container for this box: itself if inline/anon, or
    /// a trailing AnonymousBlock child if it is a block container.
    fn inline_container(&mut self) -> &mut LayoutBox<'a> {
        match &self.box_type {
            BoxType::Inline(_) | BoxType::AnonymousBlock => self,
            // An inline-block establishes its own block formatting context, so
            // its inline children belong in an anonymous block just like a
            // block container's — that is what gives them line boxes.
            BoxType::InlineBlock(_)
            | BoxType::Block(_)
            | BoxType::Flex(_)
            | BoxType::Grid(_)
            | BoxType::Table(_)
            | BoxType::TableRow(_)
            | BoxType::TableCell(_) => {
                let needs_anon = !matches!(
                    self.children.last(),
                    Some(LayoutBox {
                        box_type: BoxType::AnonymousBlock,
                        ..
                    })
                );
                if needs_anon {
                    self.children.push(LayoutBox::new(
                        BoxType::AnonymousBlock,
                        self.text_style,
                        self.text_align, // inherit text-align from parent block
                    ));
                }
                self.children.last_mut().unwrap()
            }
        }
    }

    // ── Layout entry ─────────────────────────────────────────────────────

    pub fn layout(&mut self, containing: Dimensions) {
        match &self.box_type {
            BoxType::Block(_)
            | BoxType::TableRow(_)
            | BoxType::TableCell(_)
            | BoxType::InlineBlock(_) => self.layout_block(containing),
            BoxType::Flex(_) => self.layout_flex(containing),
            BoxType::Grid(_) => self.layout_grid(containing),
            BoxType::Table(_) => self.layout_table(containing),
            BoxType::AnonymousBlock => self.layout_inline(containing),
            BoxType::Inline(_) => {}
        }
    }

    fn layout_block(&mut self, containing: Dimensions) {
        self.calc_width(containing);
        self.calc_position(containing);
        self.layout_children();
        self.calc_height();
    }

    fn layout_inline(&mut self, containing: Dimensions) {
        self.calc_width(containing);
        self.calc_position(containing);

        // Pre-layout any InlineBlock children so their dimensions are known.
        let avail = self.dimensions;
        for child in &mut self.children {
            if matches!(child.box_type, BoxType::InlineBlock(_)) {
                let ib_containing = Dimensions {
                    content: Rect {
                        x: avail.content.x,
                        y: avail.content.y,
                        width: avail.content.width,
                        height: 0.0,
                    },
                    ..Default::default()
                };
                // Lay out the inline-block's internals to get its natural dimensions.
                child.calc_width(ib_containing);
                child.calc_position(ib_containing);
                child.layout_children();
                child.calc_height();
            }
        }

        // collect_inline_pieces is called on *self* (the AnonymousBlock) so it can
        // detect InlineBlock children at the correct index level.
        let mut pieces = Vec::new();
        self.collect_inline_pieces(&mut pieces);
        self.line_boxes = build_line_boxes(
            pieces,
            self.dimensions.content.x,
            self.dimensions.content.y,
            self.dimensions.content.width,
            self.text_align,
        );
        self.dimensions.content.height = self.line_boxes.iter().map(|l| l.rect.height).sum();

        // Apply positions to InlineBlock children from their line-box placements.
        let placements: Vec<(usize, f32, f32)> = self
            .line_boxes
            .iter()
            .flat_map(|lb| lb.inline_boxes.iter().copied())
            .collect();
        for (idx, bx, by) in placements {
            self.children[idx].place_margin_box_at(bx, by);
        }
    }

    fn layout_flex(&mut self, containing: Dimensions) {
        self.calc_width(containing);
        self.calc_position(containing);
        self.layout_flex_children();
        self.calc_height();
    }

    // ── Width ─────────────────────────────────────────────────────────────

    fn calc_width(&mut self, containing: Dimensions) {
        let Some(style) = self.style() else {
            self.dimensions.content.width = containing.content.width;
            return;
        };

        let cw = containing.content.width;
        let zero = Value::Length(0.0, Unit::Px);
        let auto_kw = Value::Keyword("auto".into());

        let width_val = style.value("width").unwrap_or(&auto_kw).clone();
        let margin_l_v = style.lookup("margin-left", "margin", &zero);
        let margin_r_v = style.lookup("margin-right", "margin", &zero);
        let border_l = style.lookup("border-left-width", "border-width", &zero);
        let border_r = style.lookup("border-right-width", "border-width", &zero);
        let padding_l = style.lookup("padding-left", "padding", &zero);
        let padding_r = style.lookup("padding-right", "padding", &zero);

        let fs = get_font_size(self.style());
        let px = |v: &Value| to_px(v, cw, fs);

        let width_auto = width_val == auto_kw;
        let ml_auto = margin_l_v == auto_kw;
        let mr_auto = margin_r_v == auto_kw;

        let border_px = px(&border_l) + px(&border_r);
        let padding_px = px(&padding_l) + px(&padding_r);
        let ml_base = if ml_auto { 0.0 } else { px(&margin_l_v) };
        let mr_base = if mr_auto { 0.0 } else { px(&margin_r_v) };

        // Resolve min-width / max-width
        let min_w = style.value("min-width").map(&px).unwrap_or(0.0);
        let max_w = style
            .value("max-width")
            .map(&px)
            .filter(|&v| v > 0.0)
            .unwrap_or(f32::MAX);

        let border_box = matches!(
            style.value("box-sizing"),
            Some(Value::Keyword(s)) if s == "border-box"
        );

        // A replaced element (an image) is sized from its intrinsic dimensions
        // and the specified width/height, not from the containing block.
        if let Some((replaced_w, replaced_h)) = self.replaced_size(cw) {
            let d = &mut self.dimensions;
            d.content.width = replaced_w.max(min_w).min(max_w);
            d.margin.left = ml_base;
            d.margin.right = mr_base;
            d.border.left = px(&border_l);
            d.border.right = px(&border_r);
            d.padding.left = px(&padding_l);
            d.padding.right = px(&padding_r);
            self.replaced_height = Some(replaced_h);
            return;
        }

        let content_w = if width_auto {
            let available = f32::max(0.0, cw - ml_base - mr_base - border_px - padding_px);
            // `width: auto` fills the containing block for in-flow blocks, but an
            // inline-block shrinks to fit its content instead.
            let w = if matches!(self.box_type, BoxType::InlineBlock(_)) {
                self.shrink_to_fit_width().min(available)
            } else {
                available
            };
            w.max(min_w).min(max_w)
        } else {
            let w = px(&width_val).max(min_w).min(max_w);
            if border_box {
                (w - border_px - padding_px).max(0.0)
            } else {
                w
            }
        };

        // Distribute free space to auto margins (block centering etc.)
        let remaining = cw - content_w - ml_base - mr_base - border_px - padding_px;
        let (ml_final, mr_final) = if !width_auto {
            match (ml_auto, mr_auto) {
                (true, true) => (remaining / 2.0, remaining / 2.0),
                (true, false) => (remaining, mr_base),
                (false, true) => (ml_base, remaining),
                (false, false) => (ml_base, mr_base),
            }
        } else {
            (ml_base, mr_base)
        };

        let d = &mut self.dimensions;
        d.content.width = content_w;
        d.margin.left = ml_final;
        d.margin.right = mr_final;
        d.border.left = px(&border_l);
        d.border.right = px(&border_r);
        d.padding.left = px(&padding_l);
        d.padding.right = px(&padding_r);
    }

    // ── Position ──────────────────────────────────────────────────────────

    fn calc_position(&mut self, containing: Dimensions) {
        let zero = Value::Length(0.0, Unit::Px);
        let auto = Value::Keyword("auto".into());
        let ch = containing.content.height;
        let cw = containing.content.width;

        // Extract all values from the styled node before taking a mutable borrow.
        let (mt, mb, bt, bb, pt, pb, rel_offsets) = if let Some(style) = self.style() {
            let fs = get_font_size(Some(style));
            let mt = to_px(&style.lookup("margin-top", "margin", &zero), cw, fs);
            let mb = to_px(&style.lookup("margin-bottom", "margin", &zero), cw, fs);
            let bt = to_px(
                &style.lookup("border-top-width", "border-width", &zero),
                cw,
                fs,
            );
            let bb = to_px(
                &style.lookup("border-bottom-width", "border-width", &zero),
                cw,
                fs,
            );
            let pt = to_px(&style.lookup("padding-top", "padding", &zero), cw, fs);
            let pb = to_px(&style.lookup("padding-bottom", "padding", &zero), cw, fs);

            let rel = if style.position() == Position::Relative {
                let top = style.value("top").cloned().unwrap_or_else(|| auto.clone());
                let bottom = style
                    .value("bottom")
                    .cloned()
                    .unwrap_or_else(|| auto.clone());
                let left = style.value("left").cloned().unwrap_or_else(|| auto.clone());
                let right = style
                    .value("right")
                    .cloned()
                    .unwrap_or_else(|| auto.clone());
                let dy = if top != auto {
                    to_px(&top, cw, fs)
                } else if bottom != auto {
                    -to_px(&bottom, cw, fs)
                } else {
                    0.0
                };
                let dx = if left != auto {
                    to_px(&left, cw, fs)
                } else if right != auto {
                    -to_px(&right, cw, fs)
                } else {
                    0.0
                };
                Some((dx, dy))
            } else {
                None
            };
            (mt, mb, bt, bb, pt, pb, rel)
        } else {
            (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, None)
        };

        let d = &mut self.dimensions;
        d.margin.top = mt;
        d.margin.bottom = mb;
        d.border.top = bt;
        d.border.bottom = bb;
        d.padding.top = pt;
        d.padding.bottom = pb;

        d.content.x = containing.content.x + d.margin.left + d.border.left + d.padding.left;
        d.content.y = containing.content.y + ch + mt + bt + pt;

        if let Some((dx, dy)) = rel_offsets {
            d.content.x += dx;
            d.content.y += dy;
        }
    }

    // ── Children ──────────────────────────────────────────────────────────

    fn layout_children(&mut self) {
        // `content.height` doubles as the block-flow cursor below, so it must
        // start at zero — grid and table lay a box out more than once, and a
        // stale cursor would push the children down by the previous height.
        self.dimensions.content.height = 0.0;

        let vp_w = self.viewport_w;
        for i in 0..self.children.len() {
            // Propagate viewport width to every child.
            self.children[i].viewport_w = vp_w;

            let pos = self.children[i]
                .style()
                .map(|s| s.position())
                .unwrap_or(Position::Static);

            match pos {
                Position::Absolute => {
                    // Containing block = parent's padding box.
                    let pb = self.dimensions.padding_box();
                    let containing = Dimensions {
                        content: Rect {
                            x: pb.x,
                            y: pb.y,
                            width: pb.width,
                            height: pb.height,
                        },
                        ..Default::default()
                    };
                    self.children[i].layout_absolute(containing);
                    // Out of normal flow — don't accumulate height.
                }
                Position::Fixed => {
                    // Containing block = viewport (top-left origin, full viewport width).
                    let containing = Dimensions {
                        content: Rect {
                            x: 0.0,
                            y: 0.0,
                            width: vp_w,
                            height: 0.0,
                        },
                        ..Default::default()
                    };
                    self.children[i].layout_absolute(containing);
                    // Out of normal flow.
                }
                _ => {
                    // Collapse adjacent block margins: the gap between two sibling
                    // blocks is max(prev.margin_bottom, next.margin_top), not their sum.
                    let prev_mb = if i > 0 {
                        self.children[i - 1].dimensions.margin.bottom
                    } else {
                        0.0
                    };
                    let next_mt = self.children[i]
                        .style()
                        .map(|s| {
                            let zero = Value::Length(0.0, Unit::Px);
                            let cw = self.dimensions.content.width;
                            let fs = get_font_size(Some(s));
                            to_px(&s.lookup("margin-top", "margin", &zero), cw, fs)
                        })
                        .unwrap_or(0.0);
                    // Reduce accumulated height by the smaller of the two margins.
                    let collapse = prev_mb.min(next_mt);
                    self.dimensions.content.height -= collapse;

                    let containing = self.dimensions;
                    self.children[i].layout(containing);
                    self.dimensions.content.height +=
                        self.children[i].dimensions.margin_box().height;
                }
            }
        }
    }

    /// Lay out an absolutely positioned box relative to `containing` (its containing block).
    fn layout_absolute(&mut self, containing: Dimensions) {
        self.calc_width(containing);

        let zero = Value::Length(0.0, Unit::Px);
        let auto_kw = Value::Keyword("auto".into());
        let cw = containing.content.width;
        let ch = containing.content.height;

        // Extract edge values upfront (avoids borrow conflict with &mut self).
        let (mt, mb, bt, bb, pt, pb) = if let Some(s) = self.style() {
            let fs = get_font_size(Some(s));
            (
                to_px(&s.lookup("margin-top", "margin", &zero), cw, fs),
                to_px(&s.lookup("margin-bottom", "margin", &zero), cw, fs),
                to_px(&s.lookup("border-top-width", "border-width", &zero), cw, fs),
                to_px(
                    &s.lookup("border-bottom-width", "border-width", &zero),
                    cw,
                    fs,
                ),
                to_px(&s.lookup("padding-top", "padding", &zero), cw, fs),
                to_px(&s.lookup("padding-bottom", "padding", &zero), cw, fs),
            )
        } else {
            (0.0, 0.0, 0.0, 0.0, 0.0, 0.0)
        };

        let (top_v, bottom_v, left_v, right_v) = if let Some(s) = self.style() {
            (
                s.value("top").cloned().unwrap_or_else(|| auto_kw.clone()),
                s.value("bottom")
                    .cloned()
                    .unwrap_or_else(|| auto_kw.clone()),
                s.value("left").cloned().unwrap_or_else(|| auto_kw.clone()),
                s.value("right").cloned().unwrap_or_else(|| auto_kw.clone()),
            )
        } else {
            (
                auto_kw.clone(),
                auto_kw.clone(),
                auto_kw.clone(),
                auto_kw.clone(),
            )
        };

        self.dimensions.margin.top = mt;
        self.dimensions.margin.bottom = mb;
        self.dimensions.border.top = bt;
        self.dimensions.border.bottom = bb;
        self.dimensions.padding.top = pt;
        self.dimensions.padding.bottom = pb;

        // Layout children to determine content height.
        self.layout_children();
        self.calc_height();

        // Now compute absolute position from top/left/right/bottom offsets.
        let total_w = self.dimensions.margin_box().width;
        let total_h = self.dimensions.margin_box().height;
        let auto = &auto_kw;
        let fs = get_font_size(self.style());

        let cx = if left_v != *auto {
            containing.content.x
                + to_px(&left_v, cw, fs)
                + self.dimensions.margin.left
                + self.dimensions.border.left
                + self.dimensions.padding.left
        } else if right_v != *auto {
            containing.content.x + cw - to_px(&right_v, cw, fs) - total_w
                + self.dimensions.margin.left
                + self.dimensions.border.left
                + self.dimensions.padding.left
        } else {
            containing.content.x
                + self.dimensions.margin.left
                + self.dimensions.border.left
                + self.dimensions.padding.left
        };

        let cy = if top_v != *auto {
            containing.content.y
                + to_px(&top_v, ch, fs)
                + self.dimensions.margin.top
                + self.dimensions.border.top
                + self.dimensions.padding.top
        } else if bottom_v != *auto {
            containing.content.y + ch - to_px(&bottom_v, ch, fs) - total_h
                + self.dimensions.margin.top
                + self.dimensions.border.top
                + self.dimensions.padding.top
        } else {
            containing.content.y
                + self.dimensions.margin.top
                + self.dimensions.border.top
                + self.dimensions.padding.top
        };

        self.dimensions.content.x = cx;
        self.dimensions.content.y = cy;
    }

    // ── Flex ──────────────────────────────────────────────────────────────
    //
    //  A single-pass flexbox implementation covering the parts of CSS Flexible
    //  Box Layout that pages actually lean on: both axes and their reversed
    //  forms, wrapping, gaps, `flex-basis` (including intrinsic `auto` sizing),
    //  grow/shrink distribution, `justify-content`, and `align-items` /
    //  `align-self`.

    fn flex_direction(&self) -> FlexDirection {
        match self.style().and_then(|s| s.value("flex-direction")) {
            Some(Value::Keyword(s)) => match s.as_str() {
                "row-reverse" => FlexDirection::RowReverse,
                "column" => FlexDirection::Column,
                "column-reverse" => FlexDirection::ColumnReverse,
                _ => FlexDirection::Row,
            },
            _ => FlexDirection::Row,
        }
    }

    fn flex_wraps(&self) -> bool {
        matches!(
            self.style().and_then(|s| s.value("flex-wrap")),
            Some(Value::Keyword(s)) if s == "wrap" || s == "wrap-reverse"
        )
    }

    fn flex_justify_content(&self) -> JustifyContent {
        match self.style().and_then(|s| s.value("justify-content")) {
            Some(Value::Keyword(s)) => match s.as_str() {
                "center" => JustifyContent::Center,
                "flex-end" | "end" => JustifyContent::End,
                "space-between" => JustifyContent::SpaceBetween,
                "space-around" => JustifyContent::SpaceAround,
                "space-evenly" => JustifyContent::SpaceEvenly,
                _ => JustifyContent::Start,
            },
            _ => JustifyContent::Start,
        }
    }

    fn flex_align_items(&self) -> AlignItems {
        Self::align_keyword(self.style().and_then(|s| s.value("align-items")))
            .unwrap_or(AlignItems::Stretch) // CSS default
    }

    fn align_keyword(value: Option<&Value>) -> Option<AlignItems> {
        match value {
            Some(Value::Keyword(s)) => match s.as_str() {
                "center" => Some(AlignItems::Center),
                "flex-end" | "end" => Some(AlignItems::End),
                "flex-start" | "start" => Some(AlignItems::Start),
                "stretch" => Some(AlignItems::Stretch),
                _ => None,
            },
            _ => None,
        }
    }

    /// `align-self` on the item, falling back to the container's `align-items`.
    fn flex_align_self(&self, fallback: AlignItems) -> AlignItems {
        Self::align_keyword(self.style().and_then(|s| s.value("align-self"))).unwrap_or(fallback)
    }

    /// `(row_gap, column_gap)` in pixels.
    fn flex_gaps(&self) -> (f32, f32) {
        let Some(style) = self.style() else {
            return (0.0, 0.0);
        };
        let read = |name: &str| style.value(name).map(|v| v.to_px()).unwrap_or(0.0);
        (read("row-gap"), read("column-gap"))
    }

    /// The explicit `height` of this box in pixels, if it has one.
    fn explicit_height(&self) -> Option<f32> {
        match self.style().and_then(|s| s.value("height")) {
            Some(Value::Length(h, Unit::Px)) => Some(*h),
            _ => None,
        }
    }

    fn layout_flex_children(&mut self) {
        let direction = self.flex_direction();
        let wraps = self.flex_wraps();
        let justify = self.flex_justify_content();
        let align_items = self.flex_align_items();
        let (row_gap, column_gap) = self.flex_gaps();
        let (main_gap, cross_gap) = if direction.is_row() {
            (column_gap, row_gap)
        } else {
            (row_gap, column_gap)
        };

        let container = self.dimensions;
        if self.children.is_empty() {
            self.dimensions.content.height = self.explicit_height().unwrap_or(0.0);
            return;
        }

        // The main axis is definite for rows (the container's width) and for
        // columns only when a height was given.
        let container_main = if direction.is_row() {
            Some(container.content.width)
        } else {
            self.explicit_height()
        };
        let container_cross = if direction.is_row() {
            self.explicit_height()
        } else {
            Some(container.content.width)
        };

        // ── 1. Measure items ──────────────────────────────────────────────
        let mut items: Vec<FlexItem> = Vec::with_capacity(self.children.len());
        for (index, child) in self.children.iter_mut().enumerate() {
            // Populates margins, borders and padding along the inline axis.
            child.calc_width(container);

            let main_edges = if direction.is_row() {
                child.horizontal_edges()
            } else {
                child.vertical_edges()
            };

            let base = child.flex_base_size(direction, container, align_items);
            items.push(FlexItem {
                index,
                base,
                target: base,
                main_edges,
                grow: child.flex_factor("flex-grow", 0.0),
                shrink: child.flex_factor("flex-shrink", 1.0),
                outer_cross: 0.0,
            });
        }

        // ── 2. Break into lines ───────────────────────────────────────────
        let limit = container_main.unwrap_or(f32::INFINITY);
        let mut lines: Vec<Vec<usize>> = Vec::new();
        let mut current: Vec<usize> = Vec::new();
        let mut used = 0.0f32;
        for (i, item) in items.iter().enumerate() {
            let outer = item.base + item.main_edges;
            let gap_before = if current.is_empty() { 0.0 } else { main_gap };
            if wraps && !current.is_empty() && used + gap_before + outer > limit {
                lines.push(std::mem::take(&mut current));
                used = outer;
            } else {
                used += gap_before + outer;
            }
            current.push(i);
        }
        if !current.is_empty() {
            lines.push(current);
        }

        // ── 3. Resolve main sizes, then lay each item out ─────────────────
        for line in &lines {
            let gaps = main_gap * line.len().saturating_sub(1) as f32;
            let content: f32 = line
                .iter()
                .map(|&i| items[i].base + items[i].main_edges)
                .sum();
            let free = limit - content - gaps;
            if !free.is_finite() {
                continue; // indefinite main axis: items keep their base size
            }

            if free > 0.0 {
                let total_grow: f32 = line.iter().map(|&i| items[i].grow).sum();
                if total_grow > 0.0 {
                    for &i in line {
                        items[i].target = items[i].base + free * items[i].grow / total_grow;
                    }
                }
            } else if free < 0.0 {
                // Shrinking is weighted by the item's base size, as in the spec.
                let total_scaled: f32 = line.iter().map(|&i| items[i].base * items[i].shrink).sum();
                if total_scaled > 0.0 {
                    for &i in line {
                        let weight = items[i].base * items[i].shrink / total_scaled;
                        items[i].target = (items[i].base + free * weight).max(0.0);
                    }
                }
            }
        }

        for item in &mut items {
            let cross_available = container_cross.unwrap_or(container.content.width);
            item.outer_cross = self.children[item.index].layout_flex_item(
                direction,
                container,
                item.target,
                cross_available,
                align_items,
            );
        }

        // ── 4. Cross sizes per line, then place everything ────────────────
        let mut line_cross: Vec<f32> = lines
            .iter()
            .map(|line| {
                line.iter()
                    .map(|&i| items[i].outer_cross)
                    .fold(0.0, f32::max)
            })
            .collect();

        // `align-content: stretch` (the default): when the container has a
        // definite cross size, the lines share out whatever is left over.
        if let Some(cross_size) = container_cross {
            let used = line_cross.iter().sum::<f32>()
                + cross_gap * line_cross.len().saturating_sub(1) as f32;
            if used < cross_size && !line_cross.is_empty() {
                let extra = (cross_size - used) / line_cross.len() as f32;
                for size in &mut line_cross {
                    *size += extra;
                }
            }
        }

        let mut cross_cursor = if direction.is_row() {
            container.content.y
        } else {
            container.content.x
        };
        let main_origin = if direction.is_row() {
            container.content.x
        } else {
            container.content.y
        };

        for (line_index, line) in lines.iter().enumerate() {
            let line_size = line_cross[line_index];

            // Stretch items that have no size of their own on the cross axis.
            for &i in line {
                let child = &mut self.children[items[i].index];
                if child.flex_align_self(align_items) == AlignItems::Stretch {
                    child.stretch_cross(direction, line_size);
                    items[i].outer_cross = line_size;
                }
            }

            let gaps = main_gap * line.len().saturating_sub(1) as f32;
            let used: f32 = line
                .iter()
                .map(|&i| items[i].target + items[i].main_edges)
                .sum();
            let free = limit - used - gaps;
            let free = if free.is_finite() { free.max(0.0) } else { 0.0 };
            let (initial, extra_gap) = justify.offsets(free, line.len());

            let mut main_cursor = main_origin + initial;

            // `row-reverse` / `column-reverse` place items from the far end.
            let order: Vec<usize> = if direction.is_reverse() {
                line.iter().rev().copied().collect()
            } else {
                line.to_vec()
            };

            for &i in &order {
                let (target, main_edges, outer_cross) =
                    (items[i].target, items[i].main_edges, items[i].outer_cross);
                let child = &mut self.children[items[i].index];
                let align = child.flex_align_self(align_items);
                let cross_offset = match align {
                    AlignItems::Center => (line_size - outer_cross) / 2.0,
                    AlignItems::End => line_size - outer_cross,
                    _ => 0.0,
                };

                if direction.is_row() {
                    child.place_margin_box_at(main_cursor, cross_cursor + cross_offset);
                } else {
                    child.place_margin_box_at(cross_cursor + cross_offset, main_cursor);
                }
                main_cursor += target + main_edges + main_gap + extra_gap;
            }

            cross_cursor += line_size + cross_gap;
        }

        // ── 5. Container size along the block axis ────────────────────────
        let total_cross: f32 =
            line_cross.iter().sum::<f32>() + cross_gap * line_cross.len().saturating_sub(1) as f32;
        self.dimensions.content.height = self.explicit_height().unwrap_or(if direction.is_row() {
            total_cross
        } else {
            // For columns the main axis is vertical, so the height is the
            // longest line's content.
            lines
                .iter()
                .map(|line| {
                    let gaps = main_gap * line.len().saturating_sub(1) as f32;
                    line.iter()
                        .map(|&i| items[i].target + items[i].main_edges)
                        .sum::<f32>()
                        + gaps
                })
                .fold(0.0, f32::max)
        });
    }

    /// The flex base size: `flex-basis`, else the main-axis size property,
    /// else the item's intrinsic content size.
    fn flex_base_size(
        &mut self,
        direction: FlexDirection,
        container: Dimensions,
        align_items: AlignItems,
    ) -> f32 {
        let auto = Value::Keyword("auto".into());
        let size_property = if direction.is_row() {
            "width"
        } else {
            "height"
        };
        let percent_base = if direction.is_row() {
            container.content.width
        } else {
            container.content.height
        };
        let font_size = get_font_size(self.style());

        let explicit = self.style().and_then(|s| {
            s.value("flex-basis")
                .filter(|v| **v != auto)
                .or_else(|| s.value(size_property).filter(|v| **v != auto))
        });
        if let Some(value) = explicit {
            let px = to_px(value, percent_base, font_size);
            let border_box = matches!(
                self.style().and_then(|s| s.value("box-sizing")),
                Some(Value::Keyword(s)) if s == "border-box"
            );
            return if border_box {
                let d = self.dimensions;
                if direction.is_row() {
                    (px - d.border.left - d.border.right - d.padding.left - d.padding.right)
                        .max(0.0)
                } else {
                    (px - d.border.top - d.border.bottom - d.padding.top - d.padding.bottom)
                        .max(0.0)
                }
            } else {
                px
            };
        }

        // `auto` basis: measure the content.
        if direction.is_row() {
            (self.max_content_width() - self.horizontal_edges()).max(0.0)
        } else {
            // A column item's natural height needs a real layout pass first.
            let cross = container.content.width;
            self.layout_flex_item(direction, container, f32::NAN, cross, align_items);
            self.dimensions.content.height
        }
    }

    /// Lay this flex item out with `main` as its main-axis content size
    /// (`NaN` means "keep the natural size"). Returns its outer cross size.
    fn layout_flex_item(
        &mut self,
        direction: FlexDirection,
        container: Dimensions,
        main: f32,
        cross_available: f32,
        align_items: AlignItems,
    ) -> f32 {
        let origin = Dimensions {
            content: Rect {
                x: container.content.x,
                y: container.content.y,
                width: if direction.is_row() {
                    container.content.width
                } else {
                    cross_available
                },
                height: 0.0,
            },
            ..Default::default()
        };

        if direction.is_row() {
            let width = if main.is_nan() {
                self.dimensions.content.width
            } else {
                main
            };
            self.layout_with_assigned_width(origin, width);
            self.dimensions.margin_box().height
        } else {
            // Column: the cross axis is horizontal, so the width comes from the
            // container (when stretching) or from the item's own content.
            let stretches = self.flex_align_self(align_items) == AlignItems::Stretch;
            let d = self.dimensions;
            let cross_edges = d.margin.left
                + d.margin.right
                + d.border.left
                + d.border.right
                + d.padding.left
                + d.padding.right;
            let width = if stretches {
                (cross_available - cross_edges).max(0.0)
            } else {
                (self.max_content_width() - self.horizontal_edges()).max(0.0)
            };
            self.layout_with_assigned_width(origin, width);
            if !main.is_nan() {
                self.dimensions.content.height = main;
            }
            self.dimensions.margin_box().width
        }
    }

    /// Grow this item to fill its flex line on the cross axis.
    fn stretch_cross(&mut self, direction: FlexDirection, line_size: f32) {
        let d = self.dimensions;
        if direction.is_row() {
            // Only stretch when the item has no height of its own.
            if self.explicit_height().is_some() {
                return;
            }
            let edges = d.margin.top
                + d.margin.bottom
                + d.border.top
                + d.border.bottom
                + d.padding.top
                + d.padding.bottom;
            self.dimensions.content.height = (line_size - edges).max(d.content.height);
        } else {
            if self.style().and_then(|s| s.value("width")).is_some() {
                return;
            }
            let edges = d.margin.left
                + d.margin.right
                + d.border.left
                + d.border.right
                + d.padding.left
                + d.padding.right;
            self.dimensions.content.width = (line_size - edges).max(d.content.width);
        }
    }

    fn layout_with_assigned_width(&mut self, containing: Dimensions, width: f32) {
        match &self.box_type {
            BoxType::Block(_)
            | BoxType::Grid(_)
            | BoxType::Table(_)
            | BoxType::TableRow(_)
            | BoxType::TableCell(_) => {
                self.calc_width(containing);
                self.dimensions.content.width = width;
                self.calc_position(containing);
                self.layout_children();
                self.calc_height();
            }
            BoxType::Flex(_) => {
                self.calc_width(containing);
                self.dimensions.content.width = width;
                self.calc_position(containing);
                self.layout_flex_children();
                self.calc_height();
            }
            BoxType::AnonymousBlock => {
                self.calc_width(containing);
                self.dimensions.content.width = width;
                self.calc_position(containing);
                // Pre-layout inline-block children.
                let avail = self.dimensions;
                for child in &mut self.children {
                    if matches!(child.box_type, BoxType::InlineBlock(_)) {
                        let ib = Dimensions {
                            content: Rect {
                                x: avail.content.x,
                                y: avail.content.y,
                                width,
                                height: 0.0,
                            },
                            ..Default::default()
                        };
                        child.calc_width(ib);
                        child.calc_position(ib);
                        child.layout_children();
                        child.calc_height();
                    }
                }
                let mut pieces = Vec::new();
                self.collect_inline_pieces(&mut pieces);
                self.line_boxes = build_line_boxes(
                    pieces,
                    self.dimensions.content.x,
                    self.dimensions.content.y,
                    self.dimensions.content.width,
                    self.text_align,
                );
                self.dimensions.content.height =
                    self.line_boxes.iter().map(|l| l.rect.height).sum();
                let placements: Vec<(usize, f32, f32)> = self
                    .line_boxes
                    .iter()
                    .flat_map(|lb| lb.inline_boxes.iter().copied())
                    .collect();
                for (idx, bx, by) in placements {
                    self.children[idx].place_margin_box_at(bx, by);
                }
            }
            BoxType::Inline(_) | BoxType::InlineBlock(_) => {}
        }
    }

    fn collect_inline_pieces(&self, pieces: &mut Vec<InlinePiece>) {
        if let BoxType::Inline(style) = &self.box_type {
            if let NodeType::Text(raw) = &style.node.node_type {
                let text = apply_text_transform(raw, self.text_style.text_transform);
                pieces.push(InlinePiece {
                    text,
                    style: self.text_style,
                    no_wrap: self.text_style.no_wrap,
                    inline_box: None,
                });
            }
        }
        for (idx, child) in self.children.iter().enumerate() {
            if matches!(child.box_type, BoxType::InlineBlock(_)) {
                // Treat the pre-laid-out inline-block as an opaque box.
                let mb = child.dimensions.margin_box();
                pieces.push(InlinePiece {
                    text: String::new(),
                    style: child.text_style,
                    no_wrap: false,
                    inline_box: Some(InlineBoxPiece {
                        index: idx,
                        width: mb.width,
                        height: mb.height,
                        baseline: child.inline_baseline(),
                    }),
                });
            } else {
                child.collect_inline_pieces(pieces);
            }
        }
    }

    // ── Replaced elements (`<img>`) ───────────────────────────────────────

    /// The element behind this box, when it is a replaced element.
    fn replaced_element(&self) -> Option<&'a ElementData> {
        let element = self.style()?.node.as_element()?;
        let replaced = element.tag_name == "img"
            || element.tag_name == "canvas"
            || element.tag_name == "textarea"
            || (element.tag_name == "input" && element.input_type() != "hidden");
        replaced.then_some(element)
    }

    /// Intrinsic size of a form control, in the absence of CSS sizing.
    ///
    /// Text fields size from `size`/`cols`/`rows` in character units, and
    /// checkboxes and radios are square, the way a UA stylesheet would.
    fn control_intrinsic_size(&self, element: &ElementData) -> Option<(f32, f32)> {
        let font_size = get_font_size(self.style());
        let metrics = line_metrics(font_size);
        // An average advance is enough to turn `size`/`cols` into pixels.
        let character_width = measure_text("0", font_size).max(1.0);
        let padding = 8.0;

        match element.tag_name.as_str() {
            "canvas" => {
                let w = element
                    .get_attr("width")
                    .and_then(|s| s.trim().trim_end_matches("px").parse::<f32>().ok())
                    .unwrap_or(300.0);
                let h = element
                    .get_attr("height")
                    .and_then(|s| s.trim().trim_end_matches("px").parse::<f32>().ok())
                    .unwrap_or(150.0);
                Some((w, h))
            }
            "input" => match element.input_type().as_str() {
                "checkbox" | "radio" => {
                    let box_size = (font_size * 0.85).round().max(10.0);
                    Some((box_size, box_size))
                }
                _ => {
                    let columns = element
                        .get_attr("size")
                        .and_then(|s| s.trim().parse::<f32>().ok())
                        .unwrap_or(20.0);
                    Some((
                        columns * character_width + padding,
                        metrics.new_line_size + padding,
                    ))
                }
            },
            "textarea" => {
                let columns = element
                    .get_attr("cols")
                    .and_then(|s| s.trim().parse::<f32>().ok())
                    .unwrap_or(30.0);
                let rows = element
                    .get_attr("rows")
                    .and_then(|s| s.trim().parse::<f32>().ok())
                    .unwrap_or(3.0);
                Some((
                    columns * character_width + padding,
                    rows * metrics.new_line_size + padding,
                ))
            }
            _ => None,
        }
    }

    /// Used size of a replaced element, following CSS 2.1 §10.3.2/§10.6.2:
    /// specified sizes win, a single specified size keeps the intrinsic aspect
    /// ratio, and neither means the intrinsic size.
    fn replaced_size(&self, containing_width: f32) -> Option<(f32, f32)> {
        let element = self.replaced_element()?;
        let style = self.style()?;
        let font_size = get_font_size(Some(style));
        let auto = Value::Keyword("auto".into());

        let from_css = |name: &str| {
            style
                .value(name)
                .filter(|v| **v != auto)
                .map(|v| to_px(v, containing_width, font_size))
        };
        // HTML presentational attributes are a weaker source than CSS.
        let from_attribute = |name: &str| {
            element
                .get_attr(name)
                .and_then(|raw| raw.trim().trim_end_matches("px").parse::<f32>().ok())
        };
        let width = from_css("width").or_else(|| from_attribute("width"));
        let height = from_css("height").or_else(|| from_attribute("height"));

        let intrinsic = match self.image.as_ref() {
            Some(image) => Some((image.width as f32, image.height as f32)),
            // Form controls have an intrinsic size of their own.
            None => self.control_intrinsic_size(element),
        };
        let ratio = self.image.as_ref().and_then(|image| image.aspect_ratio());

        let size = match (width, height, intrinsic) {
            (Some(w), Some(h), _) => (w, h),
            // One specified size: keep the aspect ratio when there is one (an
            // image), otherwise keep the element's own intrinsic other size —
            // a 320px-wide text field is not 320px tall.
            (Some(w), None, _) => {
                let h = match ratio {
                    Some(ratio) => w / ratio,
                    None => intrinsic.map(|(_, height)| height).unwrap_or(w),
                };
                (w, h)
            }
            (None, Some(h), _) => {
                let w = match ratio {
                    Some(ratio) => h * ratio,
                    None => intrinsic.map(|(width, _)| width).unwrap_or(h),
                };
                (w, h)
            }
            (None, None, Some((w, h))) => (w, h),
            // No image, no control metrics and no sizes: reserve room for the
            // alt-text placeholder.
            (None, None, None) => {
                let alt = element.get_attr("alt").unwrap_or("");
                let text = placeholder_text(alt);
                let font_size = self.text_style.font_size;
                (
                    measure_text(&text, font_size) + 8.0,
                    line_metrics(font_size).new_line_size + 4.0,
                )
            }
        };
        Some((size.0.max(0.0), size.1.max(0.0)))
    }

    /// Content width this box needs when its content is not wrapped —
    /// CSS's "max-content" size, used to shrink-wrap inline-blocks.
    fn shrink_to_fit_width(&self) -> f32 {
        if let Some((width, _)) = self.replaced_size(0.0) {
            return width;
        }
        self.children
            .iter()
            .map(|c| c.max_content_width())
            .fold(0.0, f32::max)
    }

    /// Outer width this box would occupy on a single unbroken line.
    fn max_content_width(&self) -> f32 {
        if let Some((width, _)) = self.replaced_size(0.0) {
            return width + self.horizontal_edges();
        }
        let own_text = match &self.box_type {
            BoxType::Inline(style) => match &style.node.node_type {
                NodeType::Text(raw) => {
                    let text = apply_text_transform(raw, self.text_style.text_transform);
                    measure_text(&collapse_whitespace(&text), self.text_style.font_size)
                }
                _ => 0.0,
            },
            _ => 0.0,
        };

        let children = match &self.box_type {
            // Inline flow: children sit side by side on one line.
            BoxType::Inline(_) | BoxType::AnonymousBlock => self
                .children
                .iter()
                .map(|c| c.max_content_width())
                .sum::<f32>(),
            // Everything else stacks, so the widest child wins.
            _ => self
                .children
                .iter()
                .map(|c| c.max_content_width())
                .fold(0.0, f32::max),
        };

        own_text + children + self.horizontal_edges()
    }

    /// Total horizontal margin + border + padding. Percentage edges resolve
    /// against an unknown containing block here, so they count as zero.
    fn horizontal_edges(&self) -> f32 {
        self.edges(
            &["margin-left", "margin-right"],
            &["padding-left", "padding-right"],
            &["border-left-width", "border-right-width"],
        )
    }

    /// Total vertical margin + border + padding, read from the style.
    ///
    /// `Dimensions` only carries the vertical edges after `calc_position` has
    /// run, so code that needs them earlier (flex measurement) asks here.
    fn vertical_edges(&self) -> f32 {
        self.edges(
            &["margin-top", "margin-bottom"],
            &["padding-top", "padding-bottom"],
            &["border-top-width", "border-bottom-width"],
        )
    }

    fn edges(&self, margins: &[&str; 2], paddings: &[&str; 2], borders: &[&str; 2]) -> f32 {
        let Some(style) = self.style() else {
            return 0.0;
        };
        let zero = Value::Length(0.0, Unit::Px);
        let fs = get_font_size(Some(style));
        let px = |v: &Value| match v {
            Value::Length(_, Unit::Percent) => 0.0,
            other => to_px(other, 0.0, fs),
        };
        let sum = |names: &[&str; 2], shorthand: &str| -> f32 {
            names
                .iter()
                .map(|n| px(&style.lookup(n, shorthand, &zero)))
                .sum()
        };
        sum(margins, "margin") + sum(paddings, "padding") + sum(borders, "border-width")
    }

    fn flex_factor(&self, name: &str, default: f32) -> f32 {
        self.style()
            .and_then(|s| s.value(name))
            .map(number_value)
            .unwrap_or(default)
    }

    fn calc_height(&mut self) {
        if let Some(height) = self.replaced_height {
            self.dimensions.content.height = height;
            return;
        }
        if let Some(style) = self.style() {
            if let Some(Value::Length(h, Unit::Px)) = style.value("height").cloned() {
                let border_box = matches!(
                    style.value("box-sizing"),
                    Some(Value::Keyword(s)) if s == "border-box"
                );
                self.dimensions.content.height = if border_box {
                    let d = &self.dimensions;
                    (h - d.border.top - d.border.bottom - d.padding.top - d.padding.bottom).max(0.0)
                } else {
                    h
                };
            }
        }
    }

    /// Text colour used for inline content (e.g. list markers).
    pub fn text_color(&self) -> Color {
        self.text_style.color
    }
    /// Font size used for inline content.
    pub fn font_size(&self) -> f32 {
        self.text_style.font_size
    }

    fn layout_grid(&mut self, containing: Dimensions) {
        self.calc_width(containing);
        self.calc_position(containing);

        let style = match self.style() {
            Some(s) => s,
            None => return,
        };

        // Parse gap / row-gap / column-gap
        let gap = style
            .value("gap")
            .or_else(|| style.value("grid-gap"))
            .map(|v| v.to_px())
            .unwrap_or(0.0);
        let row_gap = style.value("row-gap").map(|v| v.to_px()).unwrap_or(gap);
        let col_gap = style.value("column-gap").map(|v| v.to_px()).unwrap_or(gap);

        let container_justify_items = parse_alignment_keyword(Some(style), "justify-items").unwrap_or("stretch");
        let container_align_items = parse_alignment_keyword(Some(style), "align-items").unwrap_or("stretch");

        // Parse grid-template-columns and grid-template-rows
        let col_spec = match style.value("grid-template-columns") {
            Some(Value::Keyword(s)) => s.clone(),
            _ => "1fr 1fr".to_string(),
        };
        let row_spec = match style.value("grid-template-rows") {
            Some(Value::Keyword(s)) => s.clone(),
            _ => "".to_string(),
        };

        let col_tokens = crate::css::expand_grid_template_tracks(&col_spec);
        let row_tokens = if row_spec.is_empty() {
            Vec::new()
        } else {
            crate::css::expand_grid_template_tracks(&row_spec)
        };

        let num_template_cols = col_tokens.len().max(1);

        struct ItemPlacement {
            child_idx: usize,
            col: usize,
            col_span: usize,
            row: usize,
            row_span: usize,
            justify_self: &'static str,
            align_self: &'static str,
        }

        struct PendingItem {
            child_idx: usize,
            col_req: Option<usize>,
            col_span: usize,
            row_req: Option<usize>,
            row_span: usize,
            justify_self: &'static str,
            align_self: &'static str,
        }

        let mut pending = Vec::new();
        for (i, child) in self.children.iter().enumerate() {
            let (c_req, c_span, r_req, r_span) = parse_grid_item_placement(child.style());
            let j_self = parse_alignment_keyword(child.style(), "justify-self").unwrap_or(container_justify_items);
            let a_self = parse_alignment_keyword(child.style(), "align-self").unwrap_or(container_align_items);
            pending.push(PendingItem {
                child_idx: i,
                col_req: c_req,
                col_span: c_span.max(1),
                row_req: r_req,
                row_span: r_span.max(1),
                justify_self: j_self,
                align_self: a_self,
            });
        }

        let mut placements = Vec::new();
        let mut occupied = std::collections::HashSet::<(usize, usize)>::new();

        // 1. Place items with explicit row and col
        for item in &pending {
            if let (Some(r), Some(c)) = (item.row_req, item.col_req) {
                for dr in 0..item.row_span {
                    for dc in 0..item.col_span {
                        occupied.insert((r + dr, c + dc));
                    }
                }
                placements.push(ItemPlacement {
                    child_idx: item.child_idx,
                    col: c,
                    col_span: item.col_span,
                    row: r,
                    row_span: item.row_span,
                    justify_self: item.justify_self,
                    align_self: item.align_self,
                });
            }
        }

        // 2. Place items with explicit row only
        for item in &pending {
            if item.row_req.is_some() && item.col_req.is_none() {
                let r = item.row_req.unwrap();
                let mut c = 0;
                loop {
                    let mut fits = true;
                    for dr in 0..item.row_span {
                        for dc in 0..item.col_span {
                            if occupied.contains(&(r + dr, c + dc)) {
                                fits = false;
                                break;
                            }
                        }
                        if !fits {
                            break;
                        }
                    }
                    if fits {
                        break;
                    }
                    c += 1;
                }
                for dr in 0..item.row_span {
                    for dc in 0..item.col_span {
                        occupied.insert((r + dr, c + dc));
                    }
                }
                placements.push(ItemPlacement {
                    child_idx: item.child_idx,
                    col: c,
                    col_span: item.col_span,
                    row: r,
                    row_span: item.row_span,
                    justify_self: item.justify_self,
                    align_self: item.align_self,
                });
            }
        }

        // 3. Place items with explicit col only
        for item in &pending {
            if item.row_req.is_none() && item.col_req.is_some() {
                let c = item.col_req.unwrap();
                let mut r = 0;
                loop {
                    let mut fits = true;
                    for dr in 0..item.row_span {
                        for dc in 0..item.col_span {
                            if occupied.contains(&(r + dr, c + dc)) {
                                fits = false;
                                break;
                            }
                        }
                        if !fits {
                            break;
                        }
                    }
                    if fits {
                        break;
                    }
                    r += 1;
                }
                for dr in 0..item.row_span {
                    for dc in 0..item.col_span {
                        occupied.insert((r + dr, c + dc));
                    }
                }
                placements.push(ItemPlacement {
                    child_idx: item.child_idx,
                    col: c,
                    col_span: item.col_span,
                    row: r,
                    row_span: item.row_span,
                    justify_self: item.justify_self,
                    align_self: item.align_self,
                });
            }
        }

        // 4. Place fully auto items
        let mut auto_cursor_row = 0;
        let mut auto_cursor_col = 0;
        for item in &pending {
            if item.row_req.is_none() && item.col_req.is_none() {
                loop {
                    if auto_cursor_col + item.col_span > num_template_cols && auto_cursor_col > 0 {
                        auto_cursor_col = 0;
                        auto_cursor_row += 1;
                    }
                    let mut fits = true;
                    for dr in 0..item.row_span {
                        for dc in 0..item.col_span {
                            if occupied.contains(&(auto_cursor_row + dr, auto_cursor_col + dc)) {
                                fits = false;
                                break;
                            }
                        }
                        if !fits {
                            break;
                        }
                    }
                    if fits {
                        for dr in 0..item.row_span {
                            for dc in 0..item.col_span {
                                occupied.insert((auto_cursor_row + dr, auto_cursor_col + dc));
                            }
                        }
                        placements.push(ItemPlacement {
                            child_idx: item.child_idx,
                            col: auto_cursor_col,
                            col_span: item.col_span,
                            row: auto_cursor_row,
                            row_span: item.row_span,
                            justify_self: item.justify_self,
                            align_self: item.align_self,
                        });
                        auto_cursor_col += item.col_span;
                        break;
                    }
                    auto_cursor_col += 1;
                    if auto_cursor_col >= num_template_cols {
                        auto_cursor_col = 0;
                        auto_cursor_row += 1;
                    }
                }
            }
        }

        placements.sort_by_key(|p| p.child_idx);

        // 2. Determine column widths
        let max_col_used = placements.iter().map(|p| p.col + p.col_span).max().unwrap_or(0);
        let num_cols = num_template_cols.max(max_col_used);

        let container_w = self.dimensions.content.width;
        let avail_w = (container_w - (num_cols.saturating_sub(1)) as f32 * col_gap).max(0.0);

        let mut col_widths = vec![0.0f32; num_cols];
        let mut fr_total = 0.0f32;
        let mut allocated_w = 0.0f32;

        for (i, tok) in col_tokens.iter().enumerate() {
            if i >= num_cols {
                break;
            }
            if tok.ends_with("fr") {
                let weight: f32 = tok.trim_end_matches("fr").parse().unwrap_or(1.0);
                fr_total += weight;
            } else if tok.ends_with("px") {
                let px: f32 = tok.trim_end_matches("px").parse().unwrap_or(0.0);
                col_widths[i] = px;
                allocated_w += px;
            } else if tok.ends_with('%') {
                let pct: f32 = tok.trim_end_matches('%').parse().unwrap_or(0.0) / 100.0;
                let px = avail_w * pct;
                col_widths[i] = px;
                allocated_w += px;
            } else {
                let px: f32 = tok.parse().unwrap_or(0.0);
                if px > 0.0 {
                    col_widths[i] = px;
                    allocated_w += px;
                } else {
                    fr_total += 1.0;
                }
            }
        }
        for _ in col_tokens.len()..num_cols {
            fr_total += 1.0;
        }

        let remaining_w = (avail_w - allocated_w).max(0.0);
        if fr_total > 0.0 {
            for i in 0..num_cols {
                let is_fr = if i < col_tokens.len() {
                    col_tokens[i].ends_with("fr")
                        || (!col_tokens[i].ends_with("px")
                            && !col_tokens[i].ends_with('%')
                            && col_tokens[i].parse::<f32>().is_err())
                } else {
                    true
                };
                if is_fr {
                    let weight: f32 = if i < col_tokens.len() && col_tokens[i].ends_with("fr") {
                        col_tokens[i].trim_end_matches("fr").parse().unwrap_or(1.0)
                    } else {
                        1.0
                    };
                    col_widths[i] = remaining_w * (weight / fr_total);
                }
            }
        }

        // 3. Determine row count and heights
        let max_row_used = placements.iter().map(|p| p.row + p.row_span).max().unwrap_or(0);
        let num_rows = row_tokens.len().max(max_row_used).max(1);

        let mut row_heights = vec![0.0f32; num_rows];
        for (r, tok) in row_tokens.iter().enumerate() {
            if r < num_rows {
                if tok.ends_with("px") {
                    row_heights[r] = tok.trim_end_matches("px").parse().unwrap_or(0.0);
                } else if let Ok(n) = tok.parse::<f32>() {
                    row_heights[r] = n;
                }
            }
        }

        let grid_x = self.dimensions.content.x;
        let grid_y = self.dimensions.content.y;

        // Measure children across cells
        for p in &placements {
            let cell_w = (p.col..p.col + p.col_span).map(|c| col_widths[c]).sum::<f32>()
                + col_gap * (p.col_span.saturating_sub(1) as f32);

            let cell_containing = Dimensions {
                content: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: cell_w,
                    height: 0.0,
                },
                ..Default::default()
            };

            let child = &mut self.children[p.child_idx];
            child.layout(cell_containing);
            let child_h = child.dimensions.margin_box().height;

            if p.row_span == 1 {
                row_heights[p.row] = row_heights[p.row].max(child_h);
            } else {
                let current_span_h: f32 = (p.row..p.row + p.row_span)
                    .map(|r| row_heights[r])
                    .sum::<f32>()
                    + row_gap * (p.row_span - 1) as f32;
                if child_h > current_span_h {
                    let diff = (child_h - current_span_h) / p.row_span as f32;
                    for r in p.row..p.row + p.row_span {
                        row_heights[r] += diff;
                    }
                }
            }
        }

        // 4. Final layout and alignment pass
        for p in &placements {
            let cell_x = grid_x
                + (0..p.col).map(|c| col_widths[c] + col_gap).sum::<f32>();
            let cell_w = (p.col..p.col + p.col_span).map(|c| col_widths[c]).sum::<f32>()
                + col_gap * (p.col_span.saturating_sub(1) as f32);

            let cell_y = grid_y
                + (0..p.row).map(|r| row_heights[r] + row_gap).sum::<f32>();
            let cell_h = (p.row..p.row + p.row_span).map(|r| row_heights[r]).sum::<f32>()
                + row_gap * (p.row_span.saturating_sub(1) as f32);

            let cell_containing = Dimensions {
                content: Rect {
                    x: cell_x,
                    y: cell_y,
                    width: cell_w,
                    height: 0.0,
                },
                ..Default::default()
            };

            let child = &mut self.children[p.child_idx];
            child.layout(cell_containing);

            let d = child.dimensions;
            let mb_w = d.margin_box().width;
            let mb_h = d.margin_box().height;

            let offset_x = match p.justify_self {
                "center" => ((cell_w - mb_w) / 2.0).max(0.0),
                "end" => (cell_w - mb_w).max(0.0),
                _ => 0.0,
            };

            let offset_y = match p.align_self {
                "center" => ((cell_h - mb_h) / 2.0).max(0.0),
                "end" => (cell_h - mb_h).max(0.0),
                _ => 0.0,
            };

            if p.justify_self == "stretch" && child.style().and_then(|s| s.value("width")).is_none() {
                let h_edges = d.margin.left + d.margin.right + d.border.left + d.border.right + d.padding.left + d.padding.right;
                child.dimensions.content.width = (cell_w - h_edges).max(d.content.width);
            }

            if p.align_self == "stretch" && child.style().and_then(|s| s.value("height")).is_none() {
                let v_edges = d.margin.top + d.margin.bottom + d.border.top + d.border.bottom + d.padding.top + d.padding.bottom;
                child.dimensions.content.height = (cell_h - v_edges).max(d.content.height);
            }

            child.place_margin_box_at(cell_x + offset_x, cell_y + offset_y);
        }

        let total_h: f32 = row_heights.iter().sum::<f32>()
            + (row_heights.len().saturating_sub(1)) as f32 * row_gap;
        self.dimensions.content.height = self.explicit_height().unwrap_or(total_h);
        self.calc_height();
    }

    fn layout_table(&mut self, containing: Dimensions) {
        self.calc_width(containing);
        self.calc_position(containing);

        let table_w = self.dimensions.content.width;

        // 1. Preferred column widths: the widest max-content cell in each column.
        //    Laying cells out to measure them would be wrong here — an auto-width
        //    cell fills its containing block, which would make every column as
        //    wide as the whole table.
        let mut col_widths = Vec::<f32>::new();
        for row in &self.children {
            for (col_idx, cell) in row.children.iter().enumerate() {
                let w = cell.max_content_width();
                if col_idx >= col_widths.len() {
                    col_widths.push(w);
                } else {
                    col_widths[col_idx] = col_widths[col_idx].max(w);
                }
            }
        }

        // 2. Scale the columns to the table's width, keeping their proportions.
        let preferred_total: f32 = col_widths.iter().sum();
        if preferred_total > 0.0 && table_w > 0.0 {
            let scale = table_w / preferred_total;
            for w in &mut col_widths {
                *w *= scale;
            }
        }

        let table_x = self.dimensions.content.x;
        let mut cursor_y = self.dimensions.content.y;
        let mut total_table_h = 0.0f32;

        for row in &mut self.children {
            row.dimensions.content.x = table_x;
            row.dimensions.content.y = cursor_y;
            row.dimensions.content.width = self.dimensions.content.width;

            let mut cell_x = table_x;
            let mut row_h = 0.0f32;

            for (col_idx, cell) in row.children.iter_mut().enumerate() {
                let cell_w = col_widths.get(col_idx).copied().unwrap_or(100.0);
                let cell_containing = Dimensions {
                    content: Rect {
                        x: cell_x,
                        y: cursor_y,
                        width: cell_w,
                        height: 0.0,
                    },
                    ..Default::default()
                };
                cell.layout(cell_containing);
                let h = cell.dimensions.margin_box().height;
                row_h = row_h.max(h);
                cell_x += cell_w;
            }

            // Cells stretch to the tallest cell in the row, so their backgrounds
            // and borders line up.
            for cell in row.children.iter_mut() {
                let d = cell.dimensions;
                let vertical_edges = d.margin.top
                    + d.margin.bottom
                    + d.border.top
                    + d.border.bottom
                    + d.padding.top
                    + d.padding.bottom;
                cell.dimensions.content.height = (row_h - vertical_edges).max(d.content.height);
            }

            row.dimensions.content.height = row_h;
            cursor_y += row_h;
            total_table_h += row_h;
        }

        self.dimensions.content.height = total_table_h;
        self.calc_height();
    }
}

// ── Flex helpers ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
enum FlexDirection {
    Row,
    RowReverse,
    Column,
    ColumnReverse,
}

impl FlexDirection {
    /// True when the main axis is horizontal.
    fn is_row(self) -> bool {
        matches!(self, FlexDirection::Row | FlexDirection::RowReverse)
    }

    /// True when items are placed from the far end of the main axis.
    fn is_reverse(self) -> bool {
        matches!(
            self,
            FlexDirection::RowReverse | FlexDirection::ColumnReverse
        )
    }
}

/// One item's resolved flex sizing, gathered before anything is positioned.
#[derive(Debug, Clone, Copy)]
struct FlexItem {
    /// Index of the item in the container's `children`.
    index: usize,
    /// Flex base size along the main axis (content box).
    base: f32,
    /// Main size after grow/shrink.
    target: f32,
    /// Margin + border + padding along the main axis.
    main_edges: f32,
    grow: f32,
    shrink: f32,
    /// Margin-box size along the cross axis, filled in once laid out.
    outer_cross: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum JustifyContent {
    Start,
    Center,
    End,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

impl JustifyContent {
    /// Returns `(initial_offset, per-gap)` given free space and item count.
    fn offsets(self, free: f32, n: usize) -> (f32, f32) {
        match self {
            JustifyContent::Start => (0.0, 0.0),
            JustifyContent::Center => (free / 2.0, 0.0),
            JustifyContent::End => (free, 0.0),
            JustifyContent::SpaceBetween => {
                if n <= 1 {
                    (0.0, 0.0)
                } else {
                    (0.0, free / (n - 1) as f32)
                }
            }
            JustifyContent::SpaceAround => {
                let gap = free / n as f32;
                (gap / 2.0, gap)
            }
            JustifyContent::SpaceEvenly => {
                let gap = free / (n + 1) as f32;
                (gap, gap)
            }
        }
    }
}

// ── Grid helpers ─────────────────────────────────────────────────────────────

fn parse_grid_line_val(style: Option<&StyledNode>, prop: &str) -> (Option<usize>, usize) {
    let Some(style) = style else {
        return (None, 1);
    };
    let Some(val) = style.value(prop) else {
        return (None, 1);
    };
    let s = match val {
        Value::Keyword(k) => k.trim(),
        Value::Number(n) => return (Some((*n as usize).saturating_sub(1)), 1),
        _ => return (None, 1),
    };
    if s.is_empty() || s == "auto" {
        return (None, 1);
    }
    if s.starts_with("span") {
        let span: usize = s.trim_start_matches("span").trim().parse().unwrap_or(1);
        return (None, span.max(1));
    }
    if let Ok(line) = s.parse::<usize>() {
        return (Some(line.saturating_sub(1)), 1);
    }
    (None, 1)
}

fn parse_grid_item_placement(
    style: Option<&StyledNode>,
) -> (Option<usize>, usize, Option<usize>, usize) {
    let (col_start_raw, col_start_span) = parse_grid_line_val(style, "grid-column-start");
    let (col_end_raw, col_end_span) = parse_grid_line_val(style, "grid-column-end");

    let col_span = if col_start_span > 1 {
        col_start_span
    } else if col_end_span > 1 {
        col_end_span
    } else if let (Some(s), Some(e)) = (col_start_raw, col_end_raw) {
        if e > s {
            e - s
        } else {
            1
        }
    } else {
        1
    };

    let (row_start_raw, row_start_span) = parse_grid_line_val(style, "grid-row-start");
    let (row_end_raw, row_end_span) = parse_grid_line_val(style, "grid-row-end");

    let row_span = if row_start_span > 1 {
        row_start_span
    } else if row_end_span > 1 {
        row_end_span
    } else if let (Some(s), Some(e)) = (row_start_raw, row_end_raw) {
        if e > s {
            e - s
        } else {
            1
        }
    } else {
        1
    };

    (col_start_raw, col_span, row_start_raw, row_span)
}

fn parse_alignment_keyword(style: Option<&StyledNode>, prop: &str) -> Option<&'static str> {
    match style.and_then(|s| s.value(prop)) {
        Some(Value::Keyword(k)) => match k.to_ascii_lowercase().as_str() {
            "start" | "flex-start" => Some("start"),
            "end" | "flex-end" => Some("end"),
            "center" => Some("center"),
            "stretch" => Some("stretch"),
            _ => None,
        },
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum AlignItems {
    Stretch,
    Start,
    Center,
    End,
}

// ── Tree construction ─────────────────────────────────────────────────────────

/// Supplies decoded bitmaps for replaced elements.
///
/// Layout knows nothing about URLs or caches: the document layer resolves
/// `src` against the base URL and hands the result back through this trait.
pub trait ImageSource {
    fn image_for(&self, element: &ElementData) -> Option<Rc<RasterImage>>;
}

/// An image source with nothing in it — used by `layout_tree`.
pub struct NoImages;

impl ImageSource for NoImages {
    fn image_for(&self, _element: &ElementData) -> Option<Rc<RasterImage>> {
        None
    }
}

impl<F> ImageSource for F
where
    F: Fn(&ElementData) -> Option<Rc<RasterImage>>,
{
    fn image_for(&self, element: &ElementData) -> Option<Rc<RasterImage>> {
        self(element)
    }
}

/// How a replaced element fills its box (`object-fit`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ObjectFit {
    /// Stretch to the box, ignoring the aspect ratio.
    #[default]
    Fill,
    /// Scale down until the whole image fits, letterboxing the rest.
    Contain,
    /// Scale up until the box is covered, cropping the overflow.
    Cover,
}

/// Placeholder label drawn for an image that could not be shown.
pub fn placeholder_text(alt: &str) -> String {
    if alt.trim().is_empty() {
        "[image]".to_string()
    } else {
        format!("[image: {}]", alt.trim())
    }
}

pub fn build_layout_tree<'a>(node: &'a StyledNode<'a>) -> Option<LayoutBox<'a>> {
    build_layout_tree_inner(
        node,
        TextStyle::default(),
        TextAlign::default(),
        &NoImages,
        None,
    )
}

/// Build a box tree, attaching decoded images to replaced elements.
pub fn build_layout_tree_with_images<'a>(
    node: &'a StyledNode<'a>,
    images: &dyn ImageSource,
) -> Option<LayoutBox<'a>> {
    build_layout_tree_inner(
        node,
        TextStyle::default(),
        TextAlign::default(),
        images,
        None,
    )
}

/// Build a box tree, attaching images and marking the focused element.
pub fn build_layout_tree_with<'a>(
    node: &'a StyledNode<'a>,
    images: &dyn ImageSource,
    focused: Option<ElementId>,
) -> Option<LayoutBox<'a>> {
    build_layout_tree_inner(
        node,
        TextStyle::default(),
        TextAlign::default(),
        images,
        focused,
    )
}

/// True for text nodes that contain nothing but white space.
fn is_whitespace_text(node: &StyledNode) -> bool {
    matches!(&node.node.node_type, NodeType::Text(t) if t.trim().is_empty())
}

fn text_align_for(node: &StyledNode) -> TextAlign {
    match node.value("text-align") {
        Some(Value::Keyword(s)) => match s.as_str() {
            "center" => TextAlign::Center,
            "right" => TextAlign::Right,
            _ => TextAlign::Left,
        },
        _ => TextAlign::Left,
    }
}

fn build_layout_tree_inner<'a>(
    node: &'a StyledNode<'a>,
    inherited_text_style: TextStyle,
    _inherited_text_align: TextAlign,
    images: &dyn ImageSource,
    focused: Option<ElementId>,
) -> Option<LayoutBox<'a>> {
    let text_style = text_style_for_node(node, inherited_text_style);

    // text-align is read from the styled node (inherited CSS property).
    let text_align = text_align_for(node);

    let box_type = match node.display() {
        Display::Block => BoxType::Block(node),
        Display::Flex => BoxType::Flex(node),
        Display::Grid => BoxType::Grid(node),
        Display::Table => BoxType::Table(node),
        Display::TableRow => BoxType::TableRow(node),
        Display::TableCell => BoxType::TableCell(node),
        Display::Inline => BoxType::Inline(node),
        Display::InlineBlock => BoxType::InlineBlock(node),
        Display::None => return None,
    };

    let mut root = LayoutBox::new(box_type, text_style, text_align);
    if let Some(element) = node.node.as_element() {
        if element.tag_name == "img" {
            root.image = images.image_for(element);
        } else if element.tag_name == "canvas" {
            root.image = element.canvas_image();
        }
        root.focused = focused == Some(element.element_id());
    }

    // Grid and flex containers turn each in-flow child into an item, but a text
    // run of pure white space is not rendered — without this, the white space
    // between `<div>`s would consume grid cells.
    let drops_whitespace = matches!(node.display(), Display::Grid | Display::Flex);

    // Replaced elements render their own content: an <img> has none, and a
    // <textarea>'s text is its value, painted by the control painter.
    let replaced_content = node
        .node
        .as_element()
        .is_some_and(|e| matches!(e.tag_name.as_str(), "img" | "canvas" | "input" | "textarea"));

    for child in &node.children {
        if replaced_content {
            break;
        }
        if drops_whitespace && is_whitespace_text(child) {
            continue;
        }
        match child.display() {
            Display::None => {}
            Display::Block
            | Display::Flex
            | Display::Grid
            | Display::Table
            | Display::TableRow
            | Display::TableCell => {
                if let Some(b) =
                    build_layout_tree_inner(child, text_style, text_align, images, focused)
                {
                    root.children.push(b);
                }
            }
            Display::Inline | Display::InlineBlock => {
                if let Some(b) =
                    build_layout_tree_inner(child, text_style, text_align, images, focused)
                {
                    root.inline_container().children.push(b);
                }
            }
        }
    }

    Some(root)
}

/// Build and lay out a box tree for `viewport_width` pixels.
pub fn layout_tree<'a>(root: &'a StyledNode<'a>, viewport_width: f32) -> LayoutBox<'a> {
    layout_tree_with_images(root, viewport_width, &NoImages)
}

/// Build and lay out a box tree, resolving replaced-element images through
/// `images`.
pub fn layout_tree_with_images<'a>(
    root: &'a StyledNode<'a>,
    viewport_width: f32,
    images: &dyn ImageSource,
) -> LayoutBox<'a> {
    layout_tree_with(root, viewport_width, images, None)
}

/// Build and lay out a box tree, resolving images and marking the focused
/// element so the painter can draw its focus ring and caret.
pub fn layout_tree_with<'a>(
    root: &'a StyledNode<'a>,
    viewport_width: f32,
    images: &dyn ImageSource,
    focused: Option<ElementId>,
) -> LayoutBox<'a> {
    let mut root_box = build_layout_tree_with(root, images, focused).unwrap_or_else(|| {
        LayoutBox::new(
            BoxType::AnonymousBlock,
            TextStyle::default(),
            TextAlign::default(),
        )
    });

    root_box.viewport_w = viewport_width;
    let viewport = Dimensions {
        content: Rect {
            x: 0.0,
            y: 0.0,
            width: viewport_width,
            height: 0.0,
        },
        ..Default::default()
    };
    root_box.layout(viewport);
    root_box
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn text_style_for_node(node: &StyledNode, inherited: TextStyle) -> TextStyle {
    let color = match node.value("color") {
        Some(Value::Color(c)) => *c,
        _ => inherited.color,
    };
    let font_size = match node.value("font-size") {
        Some(Value::Length(size, Unit::Px)) => *size,
        Some(Value::Length(size, Unit::Em)) => size * inherited.font_size,
        Some(Value::Length(size, Unit::Percent)) => size * inherited.font_size / 100.0,
        _ => inherited.font_size,
    };
    let line_height = match node.value("line-height") {
        Some(Value::Number(n)) => *n,
        Some(Value::Length(n, Unit::Em)) => *n,
        Some(Value::Length(n, Unit::Px)) => n / font_size,
        Some(Value::Length(n, Unit::Percent)) => n / 100.0,
        Some(Value::Keyword(s)) if s == "normal" => 1.0,
        _ => inherited.line_height,
    };
    let no_wrap = match node.value("white-space") {
        Some(Value::Keyword(s)) => matches!(s.as_str(), "nowrap" | "pre"),
        _ => inherited.no_wrap,
    };
    // text-decoration is NOT a CSS-inherited property, but we propagate it
    // through the layout TextStyle so inline children pick it up.
    let (underline, strikethrough) = match node.value("text-decoration") {
        Some(Value::Keyword(s)) => {
            let s = s.as_str();
            if s == "none" {
                (false, false)
            } else {
                (s.contains("underline"), s.contains("line-through"))
            }
        }
        None => (inherited.underline, inherited.strikethrough),
        _ => (inherited.underline, inherited.strikethrough),
    };
    let text_transform = match node.value("text-transform") {
        Some(Value::Keyword(s)) => match s.as_str() {
            "uppercase" => TextTransform::Uppercase,
            "lowercase" => TextTransform::Lowercase,
            "capitalize" => TextTransform::Capitalize,
            _ => TextTransform::None,
        },
        _ => inherited.text_transform,
    };
    TextStyle {
        color,
        font_size,
        line_height,
        no_wrap,
        underline,
        strikethrough,
        text_transform,
    }
}

#[derive(Debug, Default)]
struct OpenLine {
    fragments: Vec<TextFragment>,
    /// Boxes waiting for the line's baseline: (child index, x, baseline offset).
    inline_boxes: Vec<(usize, f32, f32)>,
    width: f32,
    /// Distance from the top of the line to its baseline.
    ascent: f32,
    /// Distance from the baseline to the bottom of the line.
    descent: f32,
    height: f32,
}

impl OpenLine {
    /// True once anything — text or an inline-block — sits on this line.
    /// Leading white space is dropped, and a line break is only useful after
    /// something has been placed.
    fn has_content(&self) -> bool {
        !self.fragments.is_empty() || !self.inline_boxes.is_empty()
    }
}

/// Where a run of lines is being laid out: the content origin, the width the
/// lines wrap at, and how each finished line is aligned inside that width.
///
/// The four travel together through every line-building step, so they move as
/// one value rather than four parallel parameters.
#[derive(Debug, Clone, Copy)]
struct LineContext {
    x: f32,
    y: f32,
    max_width: f32,
    text_align: TextAlign,
}

fn build_line_boxes(
    pieces: Vec<InlinePiece>,
    x: f32,
    y: f32,
    max_width: f32,
    text_align: TextAlign,
) -> Vec<LineBox> {
    let ctx = LineContext {
        x,
        y,
        max_width: max_width.max(0.0),
        text_align,
    };
    let mut lines = Vec::new();
    let mut line = OpenLine::default();
    let mut pending_space = false;

    for piece in pieces {
        // Inline-block piece: treat as an opaque fixed-size box.
        if let Some(box_piece) = piece.inline_box {
            let space_w = if pending_space && line.has_content() {
                measure_text(" ", piece.style.font_size)
            } else {
                0.0
            };
            if line.has_content() && line.width + space_w + box_piece.width > ctx.max_width {
                flush_line(&mut lines, &mut line, ctx);
            } else {
                // A space between text and an inline-block is a real space.
                line.width += space_w;
            }
            line.inline_boxes
                .push((box_piece.index, ctx.x + line.width, box_piece.baseline));
            line.width += box_piece.width;
            // The box hangs from the shared baseline, so it contributes an
            // ascent above it and a descent below it.
            line.ascent = line.ascent.max(box_piece.baseline);
            line.descent = line.descent.max(box_piece.height - box_piece.baseline);
            line.height = line.height.max(box_piece.height);
            pending_space = false;
            continue;
        }

        // Text piece.
        let effective_max = if piece.no_wrap {
            f32::MAX
        } else {
            ctx.max_width
        };

        for run in split_whitespace_runs(&piece.text) {
            if run.chars().all(char::is_whitespace) {
                pending_space = true;
                continue;
            }

            if pending_space && line.has_content() {
                let space_w = measure_text(" ", piece.style.font_size);
                if line.width + space_w + measure_text(&run, piece.style.font_size) > effective_max
                {
                    flush_line(&mut lines, &mut line, ctx);
                } else {
                    line.width += space_w;
                }
            }
            add_word(
                &mut lines,
                &mut line,
                &run,
                piece.style,
                LineContext {
                    max_width: effective_max,
                    ..ctx
                },
            );
            pending_space = false;
        }
    }

    flush_line(&mut lines, &mut line, ctx);
    lines
}

fn split_whitespace_runs(text: &str) -> Vec<String> {
    let mut runs = Vec::new();
    let mut current = String::new();
    let mut current_is_whitespace = None;

    for c in text.chars() {
        let is_ws = c.is_whitespace();
        if current_is_whitespace.is_some_and(|prev| prev != is_ws) {
            runs.push(std::mem::take(&mut current));
        }
        current.push(c);
        current_is_whitespace = Some(is_ws);
    }
    if !current.is_empty() {
        runs.push(current);
    }
    runs
}

fn add_word(
    lines: &mut Vec<LineBox>,
    line: &mut OpenLine,
    word: &str,
    style: TextStyle,
    ctx: LineContext,
) {
    let LineContext { x, max_width, .. } = ctx;
    let word_w = measure_text(word, style.font_size);
    if line.has_content() && line.width + word_w > max_width {
        flush_line(lines, line, ctx);
    }
    if word_w <= max_width || max_width <= 0.0 {
        add_fragment(line, word.to_string(), style, x);
        return;
    }

    let mut chunk = String::new();
    let mut chunk_w = 0.0;
    for c in word.chars() {
        let cw = measure_text(&c.to_string(), style.font_size);
        if !chunk.is_empty() && chunk_w + cw > max_width {
            add_fragment(line, std::mem::take(&mut chunk), style, x);
            flush_line(lines, line, ctx);
            chunk_w = 0.0;
        }
        chunk.push(c);
        chunk_w += cw;
    }
    if !chunk.is_empty() {
        add_fragment(line, chunk, style, x);
    }
}

fn add_fragment(line: &mut OpenLine, text: String, style: TextStyle, x: f32) {
    let metrics = line_metrics(style.font_size);
    let width = measure_text(&text, style.font_size);
    let lh = metrics.new_line_size * style.line_height;
    let ascent = metrics.ascent * style.line_height;

    // Merge into the previous fragment when this word continues the same
    // styled run on the same line. Keeping a run in one fragment is what makes
    // an underline continuous across the spaces inside a link.
    if let Some(previous) = line.fragments.last_mut() {
        // Words of one run are separated by at most a single space.
        let space_width = measure_text(" ", style.font_size);
        let gap = x + line.width - (previous.rect.x + previous.rect.width);
        let continues_run = previous.font_size == style.font_size
            && previous.color == style.color
            && previous.underline == style.underline
            && previous.strikethrough == style.strikethrough
            && gap >= -0.5
            && gap <= space_width + 0.5;
        if continues_run {
            if gap > 0.5 {
                previous.text.push(' ');
            }
            previous.text.push_str(&text);
            previous.rect.width = x + line.width + width - previous.rect.x;
            previous.rect.height = previous.rect.height.max(lh);
            line.width += width;
            line.ascent = line.ascent.max(ascent);
            line.descent = line.descent.max(lh - ascent);
            line.height = line.height.max(lh);
            return;
        }
    }

    line.fragments.push(TextFragment {
        text,
        rect: Rect {
            x: x + line.width,
            y: 0.0,
            width,
            height: lh,
        },
        baseline: 0.0,
        color: style.color,
        font_size: style.font_size,
        underline: style.underline,
        strikethrough: style.strikethrough,
    });
    line.width += width;
    line.ascent = line.ascent.max(ascent);
    line.descent = line.descent.max(lh - ascent);
    line.height = line.height.max(lh);
}

/// Collapse runs of white space the way line breaking does, keeping a single
/// leading or trailing space.
///
/// The edge spaces matter: they are what separates an inline-block from the
/// text beside it, and dropping them makes a shrink-to-fit box too narrow, so
/// its own content wraps inside it.
fn collapse_whitespace(text: &str) -> String {
    let leading = text.starts_with(char::is_whitespace);
    let trailing = text.ends_with(char::is_whitespace);
    let mut collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return if leading || trailing {
            " ".to_string()
        } else {
            collapsed
        };
    }
    if leading {
        collapsed.insert(0, ' ');
    }
    if trailing {
        collapsed.push(' ');
    }
    collapsed
}

fn apply_text_transform(text: &str, transform: TextTransform) -> String {
    match transform {
        TextTransform::None => text.to_string(),
        TextTransform::Uppercase => text.to_uppercase(),
        TextTransform::Lowercase => text.to_lowercase(),
        TextTransform::Capitalize => {
            let mut cap_next = true;
            text.chars()
                .map(|c| {
                    if c.is_whitespace() {
                        cap_next = true;
                        c
                    } else if cap_next {
                        cap_next = false;
                        c.to_uppercase().next().unwrap_or(c)
                    } else {
                        c
                    }
                })
                .collect()
        }
    }
}

fn flush_line(lines: &mut Vec<LineBox>, line: &mut OpenLine, ctx: LineContext) {
    let LineContext {
        x,
        y,
        max_width,
        text_align,
    } = ctx;
    if line.fragments.is_empty() && line.inline_boxes.is_empty() {
        return;
    }

    let mut open = std::mem::take(line);
    let line_y = y + lines.iter().map(|l| l.rect.height).sum::<f32>();
    let baseline = line_y + open.ascent;
    // A tall inline-block can push the line's bottom below the text descent.
    let height = open.height.max(open.ascent + open.descent);

    let offset = match text_align {
        TextAlign::Left => 0.0,
        TextAlign::Center => ((max_width - open.width) / 2.0).max(0.0),
        TextAlign::Right => (max_width - open.width).max(0.0),
    };

    for frag in &mut open.fragments {
        frag.rect.x += offset;
        frag.rect.y = line_y;
        frag.rect.height = height;
        frag.baseline = baseline;
    }

    // Each box is dropped so that its own baseline meets the line's.
    let inline_boxes = open
        .inline_boxes
        .iter()
        .map(|&(index, box_x, box_baseline)| (index, box_x + offset, baseline - box_baseline))
        .collect();

    lines.push(LineBox {
        rect: Rect {
            x: x + offset,
            y: line_y,
            width: open.width,
            height,
        },
        baseline,
        fragments: open.fragments,
        inline_boxes,
    });
}

fn to_px(value: &Value, containing_width: f32, font_size: f32) -> f32 {
    match value {
        Value::Length(n, Unit::Px) => *n,
        Value::Length(n, Unit::Em) => n * font_size,
        Value::Length(n, Unit::Percent) => n * containing_width / 100.0,
        Value::Calc(expr) => eval_calc(expr, containing_width, font_size),
        _ => 0.0,
    }
}

/// Extract the computed `font-size` (in px) from a styled node, defaulting to 16.
fn get_font_size(style: Option<&StyledNode<'_>>) -> f32 {
    style
        .and_then(|s| s.value("font-size"))
        .and_then(|v| {
            if let Value::Length(px, Unit::Px) = v {
                Some(*px)
            } else {
                None
            }
        })
        .unwrap_or(16.0)
}

/// Recursively evaluate a `calc()` expression given the containing block width.
fn eval_calc(expr: &CalcExpr, cw: f32, fs: f32) -> f32 {
    match expr {
        CalcExpr::Literal(n, Unit::Px) => *n,
        CalcExpr::Literal(n, Unit::Em) => n * fs,
        CalcExpr::Literal(n, Unit::Percent) => n * cw / 100.0,
        CalcExpr::Literal(n, Unit::Fr) => *n,
        CalcExpr::Percent(n) => n * cw / 100.0,
        CalcExpr::Add(a, b) => eval_calc(a, cw, fs) + eval_calc(b, cw, fs),
        CalcExpr::Sub(a, b) => eval_calc(a, cw, fs) - eval_calc(b, cw, fs),
        CalcExpr::Mul(a, b) => eval_calc(a, cw, fs) * eval_calc(b, cw, fs),
        CalcExpr::Div(a, b) => {
            let d = eval_calc(b, cw, fs);
            if d.abs() < 1e-6 {
                0.0
            } else {
                eval_calc(a, cw, fs) / d
            }
        }
    }
}

fn number_value(value: &Value) -> f32 {
    match value {
        Value::Length(n, _) => *n,
        Value::Number(n) => *n,
        Value::Keyword(s) => s.parse().unwrap_or(0.0),
        Value::Color(_) => 0.0,
        Value::LinearGradient(_) => 0.0,
        Value::BoxShadow(_) => 0.0,
        Value::Transform(_) => 0.0,
        Value::Transition(_) => 0.0,
        Value::Animation(_) => 0.0,
        Value::Var { .. } => 0.0,
        Value::Calc(expr) => eval_calc(expr, 0.0, 16.0),
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::parser::parse_css;
    use crate::html::parse_html;
    use crate::style::style_tree;

    fn layout_html_css(html: &str, css: &str, vw: f32) -> LayoutBox<'static> {
        let dom = Box::leak(Box::new(parse_html(html)));
        let ss = Box::leak(Box::new(parse_css(css)));
        let styled = Box::leak(Box::new(style_tree(dom, ss)));
        layout_tree(styled, vw)
    }

    /// First box in the tree whose styled node has tag `tag`.
    fn find_tag<'b, 'a>(b: &'b LayoutBox<'a>, tag: &str) -> Option<&'b LayoutBox<'a>> {
        let matches = b
            .styled_node()
            .and_then(|s| s.node.as_element())
            .is_some_and(|e| e.tag_name == tag);
        if matches {
            return Some(b);
        }
        b.children.iter().find_map(|c| find_tag(c, tag))
    }

    /// Every box in the tree whose styled node has tag `tag`, in tree order.
    fn find_all_tags<'b, 'a>(b: &'b LayoutBox<'a>, tag: &str, out: &mut Vec<&'b LayoutBox<'a>>) {
        if b.styled_node()
            .and_then(|s| s.node.as_element())
            .is_some_and(|e| e.tag_name == tag)
        {
            out.push(b);
        }
        for child in &b.children {
            find_all_tags(child, tag, out);
        }
    }

    /// All text fragments in a subtree, with their positions.
    fn fragments<'b>(b: &'b LayoutBox, out: &mut Vec<&'b TextFragment>) {
        for line in &b.line_boxes {
            out.extend(line.fragments.iter());
        }
        for child in &b.children {
            fragments(child, out);
        }
    }

    // ── inline-block ──────────────────────────────────────────────────────

    #[test]
    fn inline_block_shrinks_to_fit_its_text() {
        let layout = layout_html_css(
            "<div><button>Hi</button></div>",
            "div { display: block; } button { display: inline-block; }",
            800.0,
        );
        let button = find_tag(&layout, "button").expect("button box");
        let width = button.dimensions.content.width;
        assert!(width > 0.0, "inline-block should have width, got {width}");
        assert!(
            width < 200.0,
            "inline-block should shrink to fit, got {width}"
        );
    }

    #[test]
    fn inline_block_lays_out_its_text_and_gains_height() {
        let layout = layout_html_css(
            "<div><button>Click me</button></div>",
            "div { display: block; } button { display: inline-block; }",
            800.0,
        );
        let button = find_tag(&layout, "button").expect("button box");
        assert!(
            button.dimensions.content.height > 0.0,
            "inline-block with text must have height"
        );

        let mut frags = Vec::new();
        fragments(button, &mut frags);
        let text: String = frags
            .iter()
            .map(|f| f.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(text, "Click me");
    }

    #[test]
    fn inline_block_text_follows_the_box_when_aligned_right() {
        let layout = layout_html_css(
            "<div><button>End</button></div>",
            "div { display: block; text-align: right; } button { display: inline-block; }",
            800.0,
        );
        let button = find_tag(&layout, "button").expect("button box");
        let mut frags = Vec::new();
        fragments(button, &mut frags);
        let frag = frags.first().expect("button text fragment");

        // The glyphs must sit inside the button's own box, not at the line origin.
        let box_x = button.dimensions.content.x;
        assert!(
            frag.rect.x >= box_x - 1.0,
            "fragment at {} should be inside the box at {}",
            frag.rect.x,
            box_x
        );
        assert!(
            box_x > 400.0,
            "right-aligned box should be on the right, got {box_x}"
        );
    }

    #[test]
    fn words_of_one_styled_run_share_a_fragment() {
        // A multi-word link must underline continuously, which means the words
        // belong to a single fragment rather than one fragment each.
        let layout = layout_html_css(
            "<p><a href='#'>About this page</a></p>",
            "p { display: block; } a { display: inline; text-decoration: underline; }",
            800.0,
        );
        let mut frags = Vec::new();
        fragments(&layout, &mut frags);
        let underlined: Vec<&&TextFragment> = frags.iter().filter(|f| f.underline).collect();
        assert_eq!(underlined.len(), 1, "expected one run, got {underlined:?}");
        assert_eq!(underlined[0].text, "About this page");
    }

    #[test]
    fn differently_styled_words_stay_in_separate_fragments() {
        let layout = layout_html_css(
            "<p>plain <b>bold</b></p>",
            "p { display: block; } b { display: inline; color: red; }",
            800.0,
        );
        let mut frags = Vec::new();
        fragments(&layout, &mut frags);
        assert_eq!(
            frags.len(),
            2,
            "colour change must break the run: {frags:?}"
        );
    }

    /// Baseline of the first line box found in a subtree.
    fn first_baseline(b: &LayoutBox) -> Option<f32> {
        if let Some(line) = b.line_boxes.first() {
            return Some(line.baseline);
        }
        b.children.iter().find_map(first_baseline)
    }

    #[test]
    fn inline_block_text_sits_on_the_surrounding_baseline() {
        // `<code>` is an inline-block with padding: without baseline
        // alignment its text floats above the line.
        let layout = layout_html_css(
            "<p>before <code>mid</code> after</p>",
            "p { display: block; } code { display: inline-block; padding: 4px; }",
            800.0,
        );
        let code = find_tag(&layout, "code").expect("code box");
        let inner = first_baseline(code).expect("code has a line");

        let paragraph = find_tag(&layout, "p").expect("paragraph box");
        let outer = paragraph
            .children
            .iter()
            .find_map(|c| c.line_boxes.first().map(|l| l.baseline))
            .expect("paragraph line");

        assert!(
            (inner - outer).abs() < 0.5,
            "inline-block text at {inner} should share the line baseline at {outer}"
        );
    }

    // ── replaced elements (images) ────────────────────────────────────────

    fn image_layout(html: &str, css: &str, width: u32, height: u32) -> LayoutBox<'static> {
        use crate::image::RasterImage;
        let dom = Box::leak(Box::new(parse_html(html)));
        let ss = Box::leak(Box::new(parse_css(css)));
        let styled = Box::leak(Box::new(style_tree(dom, ss)));
        let image = Rc::new(RasterImage::new(
            width,
            height,
            vec![255; (width * height * 4) as usize],
        ));
        layout_tree_with_images(styled, 800.0, &move |_: &ElementData| Some(image.clone()))
    }

    #[test]
    fn image_uses_its_intrinsic_size_by_default() {
        let layout = image_layout("<img src='x.png'>", "", 120, 60);
        let img = find_tag(&layout, "img").expect("img box");
        assert_eq!(img.dimensions.content.width, 120.0);
        assert_eq!(img.dimensions.content.height, 60.0);
    }

    #[test]
    fn image_width_attribute_scales_the_height() {
        let layout = image_layout("<img src='x.png' width='60'>", "", 120, 60);
        let img = find_tag(&layout, "img").expect("img box");
        assert_eq!(img.dimensions.content.width, 60.0);
        assert_eq!(
            img.dimensions.content.height, 30.0,
            "aspect ratio preserved"
        );
    }

    #[test]
    fn image_height_attribute_scales_the_width() {
        let layout = image_layout("<img src='x.png' height='30'>", "", 120, 60);
        let img = find_tag(&layout, "img").expect("img box");
        assert_eq!(img.dimensions.content.width, 60.0);
        assert_eq!(img.dimensions.content.height, 30.0);
    }

    #[test]
    fn css_size_beats_the_html_attribute() {
        let layout = image_layout(
            "<img src='x.png' width='60'>",
            "img { width: 200px; }",
            120,
            60,
        );
        let img = find_tag(&layout, "img").expect("img box");
        assert_eq!(img.dimensions.content.width, 200.0);
        assert_eq!(img.dimensions.content.height, 100.0);
    }

    #[test]
    fn both_dimensions_specified_ignore_the_aspect_ratio() {
        let layout = image_layout("<img src='x.png' width='50' height='90'>", "", 120, 60);
        let img = find_tag(&layout, "img").expect("img box");
        assert_eq!(img.dimensions.content.width, 50.0);
        assert_eq!(img.dimensions.content.height, 90.0);
    }

    #[test]
    fn an_image_that_failed_to_load_reserves_room_for_its_alt_text() {
        let layout = layout_html_css("<img src='x.png' alt='a picture'>", "", 800.0);
        let img = find_tag(&layout, "img").expect("img box");
        assert!(img.image().is_none());
        assert!(img.dimensions.content.width > 0.0, "alt text needs room");
        assert!(img.dimensions.content.width < 400.0);
        assert!(img.dimensions.content.height > 0.0);
    }

    #[test]
    fn an_inline_image_rests_on_the_text_baseline() {
        let layout = image_layout(
            "<p>text <img src='x.png'></p>",
            "p { display: block; }",
            40,
            40,
        );
        let img = find_tag(&layout, "img").expect("img box");
        let bottom = img.dimensions.margin_box().y + img.dimensions.margin_box().height;

        let paragraph = find_tag(&layout, "p").expect("paragraph box");
        let baseline = paragraph
            .children
            .iter()
            .find_map(|c| c.line_boxes.first().map(|l| l.baseline))
            .expect("paragraph line");

        assert!(
            (bottom - baseline).abs() < 1.0,
            "image bottom {bottom} should meet the baseline {baseline}"
        );
    }

    #[test]
    fn a_tall_inline_image_grows_the_line_box() {
        let layout = image_layout(
            "<p>text <img src='x.png'></p>",
            "p { display: block; }",
            40,
            80,
        );
        let paragraph = find_tag(&layout, "p").expect("paragraph box");
        assert!(
            paragraph.dimensions.content.height >= 80.0,
            "line must be at least as tall as the image, got {}",
            paragraph.dimensions.content.height
        );
    }

    #[test]
    fn images_flow_inline_next_to_each_other() {
        let layout = image_layout(
            "<p><img src='a.png'><img src='b.png'></p>",
            "p { display: block; }",
            40,
            40,
        );
        let mut images = Vec::new();
        find_all_tags(&layout, "img", &mut images);
        assert_eq!(images.len(), 2);
        assert!(
            images[1].dimensions.content.x > images[0].dimensions.content.x,
            "two inline images should sit side by side"
        );
        assert_eq!(
            images[0].dimensions.content.y,
            images[1].dimensions.content.y
        );
    }

    // ── flexbox ───────────────────────────────────────────────────────────

    /// Lay out a flex container with three `.item` children and return their boxes.
    fn flex_items(container_css: &str, item_css: &str, html: &str, vw: f32) -> Vec<Dimensions> {
        let css = format!(
            ".flex {{ display: flex; {container_css} }} .item {{ display: block; {item_css} }}"
        );
        let layout = layout_html_css(html, &css, vw);
        let flex = find_tag(&layout, "div").expect("flex container");
        flex.children.iter().map(|c| c.dimensions).collect()
    }

    const THREE_ITEMS: &str =
        "<div class='flex'><div class='item'>one</div><div class='item'>two</div><div class='item'>three</div></div>";

    #[test]
    fn flex_items_size_to_their_content_by_default() {
        let items = flex_items("", "", THREE_ITEMS, 800.0);
        for d in &items {
            assert!(
                d.content.width > 0.0,
                "an auto-basis flex item must size to its text, got {}",
                d.content.width
            );
        }
        // They are laid side by side, not stacked.
        assert!(items[0].content.x < items[1].content.x);
        assert!(items[1].content.x < items[2].content.x);
    }

    #[test]
    fn flex_grow_shares_the_free_space() {
        let css = ".flex { display: flex; } .a, .b { display: block; } .a { flex-grow: 1; } .b { flex-grow: 3; }";
        let layout = layout_html_css(
            "<div class='flex'><div class='a'>a</div><div class='b'>b</div></div>",
            css,
            800.0,
        );
        let flex = find_tag(&layout, "div").expect("flex container");
        let (a, b) = (flex.children[0].dimensions, flex.children[1].dimensions);
        assert!((a.content.width + b.content.width - 800.0).abs() < 1.0);
        // 1:3 of the free space, on top of near-equal content bases.
        assert!(b.content.width > a.content.width * 2.0, "{a:?} vs {b:?}");
    }

    #[test]
    fn flex_shrink_pulls_oversized_items_back() {
        let css = ".flex { display: flex; } .item { display: block; width: 400px; }";
        let layout = layout_html_css(
            "<div class='flex'><div class='item'>a</div><div class='item'>b</div></div>",
            css,
            500.0,
        );
        let flex = find_tag(&layout, "div").expect("flex container");
        let total: f32 = flex
            .children
            .iter()
            .map(|c| c.dimensions.margin_box().width)
            .sum();
        assert!(total <= 501.0, "items should shrink to fit, total {total}");
    }

    #[test]
    fn flex_gap_separates_items() {
        let items = flex_items("gap: 20px;", "width: 100px;", THREE_ITEMS, 800.0);
        let first_end = items[0].content.x + items[0].content.width;
        assert!(
            (items[1].content.x - first_end - 20.0).abs() < 0.5,
            "expected a 20px gap, got {}",
            items[1].content.x - first_end
        );
    }

    #[test]
    fn flex_wrap_starts_a_new_line() {
        let items = flex_items("flex-wrap: wrap;", "width: 200px;", THREE_ITEMS, 450.0);
        // Two fit on the first line, the third wraps.
        assert!((items[0].content.y - items[1].content.y).abs() < 0.5);
        assert!(
            items[2].content.y > items[0].content.y,
            "third item should wrap"
        );
        assert!(
            (items[2].content.x - items[0].content.x).abs() < 0.5,
            "wrapped item restarts at the line start"
        );
    }

    #[test]
    fn justify_content_space_between_pushes_items_to_the_edges() {
        let items = flex_items(
            "justify-content: space-between;",
            "width: 100px;",
            THREE_ITEMS,
            800.0,
        );
        assert!(items[0].content.x < 1.0, "first item hugs the start");
        let last_end = items[2].content.x + items[2].content.width;
        assert!(
            (last_end - 800.0).abs() < 1.0,
            "last item hugs the end, ended at {last_end}"
        );
    }

    #[test]
    fn align_items_center_centres_on_the_cross_axis() {
        let css = ".flex { display: flex; align-items: center; height: 200px; } \
                   .short, .tall { display: block; width: 50px; } .tall { height: 100px; }";
        let layout = layout_html_css(
            "<div class='flex'><div class='short'>s</div><div class='tall'>t</div></div>",
            css,
            400.0,
        );
        let flex = find_tag(&layout, "div").expect("flex container");
        let short = flex.children[0].dimensions.margin_box();
        let tall = flex.children[1].dimensions.margin_box();
        let short_centre = short.y + short.height / 2.0;
        let tall_centre = tall.y + tall.height / 2.0;
        assert!(
            (short_centre - tall_centre).abs() < 1.0,
            "items should share a centre line: {short_centre} vs {tall_centre}"
        );
    }

    #[test]
    fn align_self_overrides_align_items() {
        let css = ".flex { display: flex; align-items: center; height: 200px; } \
                   .a, .b { display: block; width: 40px; height: 40px; } .b { align-self: flex-start; }";
        let layout = layout_html_css(
            "<div class='flex'><div class='a'>a</div><div class='b'>b</div></div>",
            css,
            400.0,
        );
        let flex = find_tag(&layout, "div").expect("flex container");
        let a = flex.children[0].dimensions.margin_box();
        let b = flex.children[1].dimensions.margin_box();
        assert!(
            b.y < a.y,
            "align-self: flex-start should sit above the centred item"
        );
    }

    #[test]
    fn row_reverse_places_items_from_the_end() {
        let items = flex_items(
            "flex-direction: row-reverse;",
            "width: 100px;",
            THREE_ITEMS,
            800.0,
        );
        // Document order one, two, three — reversed on screen.
        assert!(items[0].content.x > items[1].content.x);
        assert!(items[1].content.x > items[2].content.x);
    }

    #[test]
    fn flex_column_stacks_items_and_applies_the_gap() {
        let items = flex_items("flex-direction: column; gap: 10px;", "", THREE_ITEMS, 400.0);
        for pair in items.windows(2) {
            let (above, below) = (pair[0], pair[1]);
            let gap = below.margin_box().y - (above.margin_box().y + above.margin_box().height);
            assert!(
                (gap - 10.0).abs() < 0.5,
                "expected a 10px column gap, got {gap}"
            );
        }
    }

    #[test]
    fn flex_item_text_moves_with_its_box() {
        // Items are sized before their position is known, so the text inside
        // has to be shifted along with the box.
        let items = flex_items(
            "justify-content: flex-end;",
            "width: 120px;",
            THREE_ITEMS,
            800.0,
        );
        let last_x = items[2].content.x;
        assert!(
            last_x > 600.0,
            "last item should be at the end, got {last_x}"
        );

        let css = ".flex { display: flex; justify-content: flex-end; } .item { display: block; width: 120px; }";
        let layout = layout_html_css(THREE_ITEMS, css, 800.0);
        let flex = find_tag(&layout, "div").expect("flex container");
        let mut frags = Vec::new();
        fragments(&flex.children[2], &mut frags);
        let frag = frags.first().expect("text in the last item");
        assert!(
            frag.rect.x >= last_x - 1.0,
            "text at {} should follow its box at {}",
            frag.rect.x,
            last_x
        );
    }

    #[test]
    fn flex_container_height_covers_its_items() {
        let css = ".flex { display: flex; } .item { display: block; height: 60px; width: 50px; }";
        let layout = layout_html_css(THREE_ITEMS, css, 800.0);
        let flex = find_tag(&layout, "div").expect("flex container");
        assert!(
            (flex.dimensions.content.height - 60.0).abs() < 1.0,
            "container should be as tall as its line, got {}",
            flex.dimensions.content.height
        );
    }

    #[test]
    fn flex_ignores_whitespace_between_items() {
        let items = flex_items(
            "",
            "width: 100px;",
            "<div class='flex'>\n  <div class='item'>a</div>\n  <div class='item'>b</div>\n</div>",
            800.0,
        );
        assert_eq!(items.len(), 2, "white space must not become a flex item");
    }

    // ── grid ──────────────────────────────────────────────────────────────

    #[test]
    fn grid_ignores_whitespace_between_items() {
        // The newlines between the divs must not consume grid cells.
        let layout = layout_html_css(
            "<div class='g'>\n  <div class='i'>a</div>\n  <div class='i'>b</div>\n  <div class='i'>c</div>\n</div>",
            ".g { display: grid; grid-template-columns: 1fr 1fr 1fr; } .i { display: block; }",
            900.0,
        );
        let grid = find_tag(&layout, "div").expect("grid box");
        let items: Vec<&LayoutBox> = grid.children.iter().collect();
        assert_eq!(items.len(), 3, "grid should hold exactly three items");

        // All three sit on one row, in three distinct columns.
        let ys: Vec<f32> = items.iter().map(|i| i.dimensions.content.y).collect();
        assert!(
            ys.iter().all(|y| (*y - ys[0]).abs() < 0.5),
            "items should share a row, got {ys:?}"
        );
        let xs: Vec<f32> = items.iter().map(|i| i.dimensions.content.x).collect();
        assert!(
            xs[0] < xs[1] && xs[1] < xs[2],
            "items should be in column order, got {xs:?}"
        );
    }

    #[test]
    fn grid_item_content_starts_at_the_top_of_its_cell() {
        // Grid lays each item out twice; the second pass must not stack the
        // children below the height measured in the first.
        let layout = layout_html_css(
            "<div class='g'><div class='i'><p>x</p></div><div class='i'><p>tall</p><p>er</p></div></div>",
            ".g { display: grid; grid-template-columns: 1fr 1fr; } .i, p { display: block; }",
            800.0,
        );
        let grid = find_tag(&layout, "div").expect("grid box");
        let item = &grid.children[0];
        let mut paragraphs = Vec::new();
        find_all_tags(item, "p", &mut paragraphs);
        let first = paragraphs.first().expect("paragraph in first item");

        let delta = first.dimensions.content.y - item.dimensions.content.y;
        assert!(
            delta.abs() < 1.0,
            "content should start at the cell top, but is {delta} px below it"
        );
    }

    #[test]
    fn grid_items_stretch_to_the_row_height() {
        let layout = layout_html_css(
            "<div class='g'><div class='i'><p>one</p></div><div class='i'><p>a</p><p>b</p><p>c</p></div></div>",
            ".g { display: grid; grid-template-columns: 1fr 1fr; } .i, p { display: block; }",
            800.0,
        );
        let grid = find_tag(&layout, "div").expect("grid box");
        let heights: Vec<f32> = grid
            .children
            .iter()
            .map(|c| c.dimensions.margin_box().height)
            .collect();
        assert_eq!(heights.len(), 2);
        assert!(
            (heights[0] - heights[1]).abs() < 1.0,
            "grid items should stretch to equal height, got {heights:?}"
        );
    }

    // ── tables ────────────────────────────────────────────────────────────

    #[test]
    fn table_columns_share_the_table_width() {
        let layout = layout_html_css(
            "<table><tr><td>a</td><td>b</td></tr></table>",
            "table { display: table; width: 400px; } tr { display: table-row; } td { display: table-cell; }",
            800.0,
        );
        let table = find_tag(&layout, "table").expect("table box");
        let mut cells = Vec::new();
        find_all_tags(table, "td", &mut cells);
        assert_eq!(cells.len(), 2);

        let total: f32 = cells.iter().map(|c| c.dimensions.margin_box().width).sum();
        assert!(
            (total - 400.0).abs() < 1.0,
            "columns should tile the table width, got {total}"
        );
        // Neither column may claim the whole table.
        for cell in &cells {
            assert!(cell.dimensions.content.width < 400.0);
        }
    }

    #[test]
    fn wider_table_content_gets_a_wider_column() {
        let layout = layout_html_css(
            "<table><tr><td>i</td><td>a much longer cell of text</td></tr></table>",
            "table { display: table; width: 600px; } tr { display: table-row; } td { display: table-cell; }",
            800.0,
        );
        let table = find_tag(&layout, "table").expect("table box");
        let mut cells = Vec::new();
        find_all_tags(table, "td", &mut cells);
        assert!(
            cells[1].dimensions.content.width > cells[0].dimensions.content.width,
            "the column with more text should be wider"
        );
    }

    #[test]
    fn table_cells_stretch_to_the_row_height() {
        let layout = layout_html_css(
            "<table><tr><td>one line</td><td><p>a</p><p>b</p></td></tr></table>",
            "table { display: table; width: 500px; } tr { display: table-row; } td { display: table-cell; } p { display: block; }",
            800.0,
        );
        let table = find_tag(&layout, "table").expect("table box");
        let mut cells = Vec::new();
        find_all_tags(table, "td", &mut cells);
        let heights: Vec<f32> = cells
            .iter()
            .map(|c| c.dimensions.border_box().height)
            .collect();
        assert!(
            (heights[0] - heights[1]).abs() < 1.0,
            "cells in a row should share its height, got {heights:?}"
        );
    }

    #[test]
    fn block_fills_viewport() {
        let layout = layout_html_css("<div></div>", "div { display: block; }", 800.0);
        fn find_width(b: &LayoutBox) -> Option<f32> {
            if b.dimensions.content.width == 800.0 {
                return Some(800.0);
            }
            b.children.iter().find_map(find_width)
        }
        assert!(find_width(&layout).is_some());
    }

    #[test]
    fn explicit_height_respected() {
        let layout = layout_html_css(
            "<div></div>",
            "div { display: block; height: 200px; }",
            800.0,
        );
        fn find_height(b: &LayoutBox) -> Option<f32> {
            if b.dimensions.content.height == 200.0 {
                return Some(200.0);
            }
            b.children.iter().find_map(find_height)
        }
        assert!(find_height(&layout).is_some());
    }

    #[test]
    fn padding_shrinks_content_width() {
        let layout = layout_html_css(
            "<div></div>",
            "div { display: block; padding: 10px; }",
            800.0,
        );
        fn find_780(b: &LayoutBox) -> bool {
            if (b.dimensions.content.width - 780.0).abs() < 0.01 {
                return true;
            }
            b.children.iter().any(find_780)
        }
        assert!(find_780(&layout));
    }

    #[test]
    fn inline_text_wraps_into_line_boxes() {
        let layout = layout_html_css(
            "<p>one two three four five</p>",
            "p { display: block; width: 55px; font-size: 16px; }",
            200.0,
        );
        fn has_wrapped_lines(lb: &LayoutBox) -> bool {
            lb.line_boxes.len() > 1 || lb.children.iter().any(has_wrapped_lines)
        }
        assert!(has_wrapped_lines(&layout));
    }

    #[test]
    fn inline_text_inherits_color_and_font_size() {
        let layout = layout_html_css(
            "<p>Hello</p>",
            "p { display: block; color: red; font-size: 24px; }",
            200.0,
        );
        fn first_fragment<'b>(lb: &'b LayoutBox<'_>) -> Option<&'b TextFragment> {
            lb.line_boxes
                .iter()
                .flat_map(|l| &l.fragments)
                .next()
                .or_else(|| lb.children.iter().find_map(first_fragment))
        }
        let frag = first_fragment(&layout).expect("expected text fragment");
        assert_eq!(frag.color, Color::rgb(255, 0, 0));
        assert_eq!(frag.font_size, 24.0);
    }

    #[test]
    fn flex_grow_distributes_free_space() {
        let layout = layout_html_css(
            r#"<div class="flex"><div class="a"></div><div class="b"></div></div>"#,
            r#"
                .flex { display: flex; width: 300px; }
                .a { display: block; width: 50px; flex-grow: 1; }
                .b { display: block; width: 50px; flex-grow: 2; }
            "#,
            300.0,
        );
        let flex = find_flex(&layout).expect("expected flex layout box");
        assert!((flex.children[0].dimensions.content.width - 116.666).abs() < 0.01);
        assert!((flex.children[1].dimensions.content.width - 183.333).abs() < 0.01);
    }

    #[test]
    fn flex_shrink_removes_overflow() {
        let layout = layout_html_css(
            r#"<div class="flex"><div class="a"></div><div class="b"></div></div>"#,
            r#"
                .flex { display: flex; width: 100px; }
                .a, .b { display: block; width: 80px; flex-shrink: 1; }
            "#,
            100.0,
        );
        let flex = find_flex(&layout).expect("expected flex layout box");
        assert!((flex.children[0].dimensions.content.width - 50.0).abs() < 0.01);
        assert!((flex.children[1].dimensions.content.width - 50.0).abs() < 0.01);
    }

    #[test]
    fn margin_auto_centers_block() {
        let layout = layout_html_css(
            "<div></div>",
            "div { display: block; width: 400px; margin: 0 auto; }",
            800.0,
        );
        fn find_div<'a>(b: &'a LayoutBox<'a>) -> Option<&'a LayoutBox<'a>> {
            if b.dimensions.content.width == 400.0 {
                return Some(b);
            }
            b.children.iter().find_map(find_div)
        }
        let div = find_div(&layout).expect("expected 400px div");
        // centered in 800px: margin left = right = 200
        assert!((div.dimensions.margin.left - 200.0).abs() < 0.01);
        assert!((div.dimensions.margin.right - 200.0).abs() < 0.01);
    }

    #[test]
    fn max_width_clamps_content() {
        let layout = layout_html_css(
            "<div></div>",
            "div { display: block; max-width: 300px; }",
            800.0,
        );
        fn find_clamped(b: &LayoutBox) -> bool {
            if (b.dimensions.content.width - 300.0).abs() < 0.01 {
                return true;
            }
            b.children.iter().any(find_clamped)
        }
        assert!(find_clamped(&layout));
    }

    #[test]
    fn min_width_expands_content() {
        let layout = layout_html_css(
            "<div></div>",
            "div { display: block; width: 50px; min-width: 200px; }",
            800.0,
        );
        fn find_min(b: &LayoutBox) -> bool {
            if (b.dimensions.content.width - 200.0).abs() < 0.01 {
                return true;
            }
            b.children.iter().any(find_min)
        }
        assert!(find_min(&layout));
    }

    #[test]
    fn text_align_center_offsets_fragments() {
        // 200px container, "Hi" is narrower than 200px; with center align
        // the fragment x should be > 0.
        let layout = layout_html_css(
            "<p>Hi</p>",
            "p { display: block; width: 200px; text-align: center; }",
            200.0,
        );
        fn first_fragment_x(lb: &LayoutBox) -> Option<f32> {
            lb.line_boxes
                .iter()
                .flat_map(|l| &l.fragments)
                .next()
                .map(|f| f.rect.x)
                .or_else(|| lb.children.iter().find_map(first_fragment_x))
        }
        let x = first_fragment_x(&layout).expect("expected fragment");
        assert!(
            x > 0.0,
            "center-aligned fragment should have x > 0, got {x}"
        );
    }

    #[test]
    fn relative_position_offsets_box() {
        let layout = layout_html_css(
            "<div></div>",
            "div { display: block; position: relative; top: 20px; left: 10px; height: 50px; }",
            800.0,
        );
        fn find_positioned<'a>(b: &'a LayoutBox<'a>) -> Option<&'a LayoutBox<'a>> {
            if b.dimensions.content.y == 20.0 && b.dimensions.content.x == 10.0 {
                return Some(b);
            }
            b.children.iter().find_map(find_positioned)
        }
        assert!(
            find_positioned(&layout).is_some(),
            "positioned box not found at (10,20)"
        );
    }

    #[test]
    fn line_height_scales_line_box() {
        // line-height: 2 should approximately double the line height.
        let base = layout_html_css(
            "<p>Hello</p>",
            "p { display: block; font-size: 16px; line-height: 1; }",
            200.0,
        );
        let tall = layout_html_css(
            "<p>Hello</p>",
            "p { display: block; font-size: 16px; line-height: 2; }",
            200.0,
        );
        fn first_line_h(lb: &LayoutBox) -> Option<f32> {
            lb.line_boxes
                .first()
                .map(|l| l.rect.height)
                .or_else(|| lb.children.iter().find_map(first_line_h))
        }
        let h1 = first_line_h(&base).expect("no line");
        let h2 = first_line_h(&tall).expect("no line");
        assert!(
            h2 > h1 * 1.5,
            "line-height:2 should roughly double line height ({h1} vs {h2})"
        );
    }

    #[test]
    fn white_space_nowrap_prevents_wrap() {
        let wrap = layout_html_css(
            "<p>one two three four five six seven eight nine ten</p>",
            "p { display: block; width: 80px; font-size: 16px; }",
            200.0,
        );
        let nowrap = layout_html_css(
            "<p>one two three four five six seven eight nine ten</p>",
            "p { display: block; width: 80px; font-size: 16px; white-space: nowrap; }",
            200.0,
        );
        fn line_count(lb: &LayoutBox) -> usize {
            lb.line_boxes.len() + lb.children.iter().map(line_count).sum::<usize>()
        }
        assert!(line_count(&wrap) > 1, "should wrap without nowrap");
        assert_eq!(line_count(&nowrap), 1, "should be one line with nowrap");
    }

    #[test]
    fn absolute_position_uses_top_left() {
        let layout = layout_html_css(
            r#"<div class="c"><div class="a"></div></div>"#,
            r#"
                .c { display: block; width: 400px; height: 300px; padding: 0; }
                .a { display: block; position: absolute; top: 50px; left: 30px; width: 100px; height: 50px; }
            "#,
            400.0,
        );
        fn find_abs<'a>(lb: &'a LayoutBox<'a>) -> Option<&'a LayoutBox<'a>> {
            if let Some(s) = lb.style() {
                if s.position() == crate::style::Position::Absolute {
                    return Some(lb);
                }
            }
            lb.children.iter().find_map(find_abs)
        }
        let abs_box = find_abs(&layout).expect("absolute box not found");
        assert!(
            (abs_box.dimensions.content.y - 50.0).abs() < 1.0,
            "expected y≈50, got {}",
            abs_box.dimensions.content.y
        );
        assert!(
            (abs_box.dimensions.content.x - 30.0).abs() < 1.0,
            "expected x≈30, got {}",
            abs_box.dimensions.content.x
        );
    }

    #[test]
    fn flex_column_stacks_vertically() {
        let layout = layout_html_css(
            r#"<div class="col"><div class="a"></div><div class="b"></div></div>"#,
            r#"
                .col { display: flex; flex-direction: column; width: 100px; }
                .a, .b { display: block; height: 40px; }
            "#,
            100.0,
        );
        let flex = find_flex(&layout).expect("flex box not found");
        // Second child should be below the first.
        assert!(
            flex.children[1].dimensions.content.y > flex.children[0].dimensions.content.y,
            "column: second child should have greater y"
        );
    }

    #[test]
    fn box_sizing_border_box_shrinks_content() {
        // width:200px border-box with padding:20px → content should be 160px
        let layout = layout_html_css(
            "<div></div>",
            "div { display: block; width: 200px; padding: 20px; box-sizing: border-box; }",
            800.0,
        );
        fn find_160(b: &LayoutBox) -> bool {
            if (b.dimensions.content.width - 160.0).abs() < 0.5 {
                return true;
            }
            b.children.iter().any(find_160)
        }
        assert!(
            find_160(&layout),
            "border-box: content width should be 200 - 40 = 160"
        );
    }

    #[test]
    fn text_transform_uppercase() {
        let layout = layout_html_css(
            "<p>hello</p>",
            "p { display: block; text-transform: uppercase; }",
            200.0,
        );
        fn has_upper(lb: &LayoutBox) -> bool {
            lb.line_boxes.iter().flat_map(|l| &l.fragments).any(|f| {
                f.text
                    .chars()
                    .all(|c| !c.is_alphabetic() || c.is_uppercase())
            }) || lb.children.iter().any(has_upper)
        }
        assert!(has_upper(&layout), "text should be uppercased");
    }

    #[test]
    fn text_decoration_underline_propagates() {
        let layout = layout_html_css(
            "<a>link</a>",
            "a { display: inline; text-decoration: underline; }",
            200.0,
        );
        fn has_underline(lb: &LayoutBox) -> bool {
            lb.line_boxes
                .iter()
                .flat_map(|l| &l.fragments)
                .any(|f| f.underline)
                || lb.children.iter().any(has_underline)
        }
        assert!(
            has_underline(&layout),
            "fragment should have underline=true"
        );
    }

    #[test]
    fn position_fixed_uses_viewport_as_containing_block() {
        // Fixed element should be positioned from (0,0), not from parent.
        let layout = layout_html_css(
            r#"<div class="outer"><div class="fixed"></div></div>"#,
            r#"
                .outer { display: block; margin: 100px; padding: 50px; width: 400px; }
                .fixed { display: block; position: fixed; top: 20px; left: 30px; width: 100px; height: 50px; }
            "#,
            800.0,
        );
        fn find_fixed<'a>(lb: &'a LayoutBox<'a>) -> Option<&'a LayoutBox<'a>> {
            if let Some(s) = lb.style() {
                if s.position() == crate::style::Position::Fixed {
                    return Some(lb);
                }
            }
            lb.children.iter().find_map(find_fixed)
        }
        let fixed = find_fixed(&layout).expect("fixed box not found");
        // top:20, left:30 — relative to viewport (0,0), not to the parent
        assert!(
            (fixed.dimensions.content.y - 20.0).abs() < 1.0,
            "fixed y should be 20, got {}",
            fixed.dimensions.content.y
        );
        assert!(
            (fixed.dimensions.content.x - 30.0).abs() < 1.0,
            "fixed x should be 30, got {}",
            fixed.dimensions.content.x
        );
    }

    #[test]
    fn margin_collapsing_reduces_gap_between_siblings() {
        // Two blocks, one with margin-bottom: 30px, next with margin-top: 20px.
        // The gap should be max(30,20)=30, not 30+20=50.
        // With collapsing, the second block's y = first_block_height + 30.
        let layout = layout_html_css(
            r#"<div class="a"></div><div class="b"></div>"#,
            r#"
                div { display: block; width: 200px; height: 40px; }
                .a { margin-bottom: 30px; }
                .b { margin-top: 20px; }
            "#,
            400.0,
        );
        fn find_divs<'a>(lb: &'a LayoutBox<'a>, out: &mut Vec<f32>) {
            if let Some(s) = lb.style() {
                if let crate::dom::NodeType::Element(e) = &s.node.node_type {
                    if e.tag_name == "div" {
                        out.push(lb.dimensions.content.y);
                    }
                }
            }
            for c in &lb.children {
                find_divs(c, out);
            }
        }
        let mut ys = Vec::new();
        find_divs(&layout, &mut ys);
        assert_eq!(ys.len(), 2, "expected 2 divs");
        // gap = y[1] - (y[0] + height[0]) should be 30 (collapsed), not 50
        let gap = ys[1] - (ys[0] + 40.0);
        assert!(
            (gap - 30.0).abs() < 1.0,
            "collapsed margin gap should be 30, got {gap}"
        );
    }

    #[test]
    fn inline_block_placed_side_by_side() {
        // Two inline-blocks of 100px each inside a 400px container.
        // They should sit on the same line with different x positions.
        let layout = layout_html_css(
            r#"<div><span class="a"></span><span class="b"></span></div>"#,
            r#"
                div { display: block; width: 400px; }
                .a, .b { display: inline-block; width: 100px; height: 50px; }
            "#,
            400.0,
        );
        fn find_inline_blocks<'a, 'b>(lb: &'b LayoutBox<'a>, out: &mut Vec<f32>) {
            if matches!(lb.box_type, BoxType::InlineBlock(_)) {
                out.push(lb.dimensions.content.x);
            }
            for c in &lb.children {
                find_inline_blocks(c, out);
            }
        }
        let mut xs = Vec::new();
        find_inline_blocks(&layout, &mut xs);
        assert_eq!(
            xs.len(),
            2,
            "expected 2 inline-block boxes, got {}",
            xs.len()
        );
        assert!(
            xs[1] > xs[0],
            "second inline-block should be to the right of the first"
        );
    }

    fn find_flex<'a, 'b>(lb: &'b LayoutBox<'a>) -> Option<&'b LayoutBox<'a>> {
        if matches!(lb.box_type, BoxType::Flex(_)) {
            return Some(lb);
        }
        lb.children.iter().find_map(find_flex)
    }

    #[test]
    fn grid_layout_distributes_fr_tracks() {
        let layout = layout_html_css(
            r#"<div class="grid"><div class="c1"></div><div class="c2"></div></div>"#,
            r#"
                .grid { display: grid; grid-template-columns: 1fr 2fr; width: 300px; gap: 0px; }
                .c1, .c2 { height: 50px; }
            "#,
            300.0,
        );
        fn find_grid<'a, 'b>(lb: &'b LayoutBox<'a>) -> Option<&'b LayoutBox<'a>> {
            if matches!(lb.box_type, BoxType::Grid(_)) {
                return Some(lb);
            }
            lb.children.iter().find_map(find_grid)
        }
        let grid = find_grid(&layout).expect("grid box not found");
        assert_eq!(grid.children.len(), 2);
        assert!((grid.children[0].dimensions.content.width - 100.0).abs() < 1.0);
        assert!((grid.children[1].dimensions.content.width - 200.0).abs() < 1.0);
    }

    #[test]
    fn table_layout_computes_column_widths() {
        let layout = layout_html_css(
            r#"<table><tr><td class="c1">A</td><td class="c2">B</td></tr></table>"#,
            r#"
                table { display: table; width: 400px; }
                tr { display: table-row; }
                td { display: table-cell; }
                .c1 { width: 150px; }
                .c2 { width: 250px; }
            "#,
            400.0,
        );
        fn find_table<'a, 'b>(lb: &'b LayoutBox<'a>) -> Option<&'b LayoutBox<'a>> {
            if matches!(lb.box_type, BoxType::Table(_)) {
                return Some(lb);
            }
            lb.children.iter().find_map(find_table)
        }
        let tbl = find_table(&layout).expect("table box not found");
        assert_eq!(tbl.children.len(), 1);
        let row = &tbl.children[0];
        assert_eq!(row.children.len(), 2);
    }
}
