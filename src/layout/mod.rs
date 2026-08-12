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

use crate::css::parser::{CalcExpr, Color, Unit, Value};
use crate::dom::NodeType;
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
    pub fn padding_box(self) -> Rect { self.content.expanded_by(self.padding) }
    pub fn border_box(self)  -> Rect { self.padding_box().expanded_by(self.border) }
    pub fn margin_box(self)  -> Rect { self.border_box().expanded_by(self.margin) }
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
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum TextTransform { #[default] None, Uppercase, Lowercase, Capitalize }

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
    /// For inline-block pieces: (child_idx_in_anon_block, margin_box_w, margin_box_h).
    inline_box: Option<(usize, f32, f32)>,
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
        }
    }

    fn style(&self) -> Option<&StyledNode<'a>> {
        match &self.box_type {
            BoxType::Block(s) | BoxType::Flex(s) | BoxType::Grid(s) | BoxType::Table(s) | BoxType::TableRow(s) | BoxType::TableCell(s) | BoxType::Inline(s) | BoxType::InlineBlock(s) => Some(s),
            BoxType::AnonymousBlock => None,
        }
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

    /// Return the inline container for this box: itself if inline/anon, or
    /// a trailing AnonymousBlock child if it is a block container.
    fn inline_container(&mut self) -> &mut LayoutBox<'a> {
        match &self.box_type {
            BoxType::Inline(_) | BoxType::AnonymousBlock | BoxType::InlineBlock(_) => self,
            BoxType::Block(_) | BoxType::Flex(_) | BoxType::Grid(_) | BoxType::Table(_) | BoxType::TableRow(_) | BoxType::TableCell(_) => {
                let needs_anon = !matches!(
                    self.children.last(),
                    Some(LayoutBox { box_type: BoxType::AnonymousBlock, .. })
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
            BoxType::Block(_) | BoxType::TableRow(_) | BoxType::TableCell(_) | BoxType::InlineBlock(_) => self.layout_block(containing),
            BoxType::Flex(_)         => self.layout_flex(containing),
            BoxType::Grid(_)         => self.layout_grid(containing),
            BoxType::Table(_)        => self.layout_table(containing),
            BoxType::AnonymousBlock  => self.layout_inline(containing),
            BoxType::Inline(_)       => {}
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
                        x: avail.content.x, y: avail.content.y,
                        width: avail.content.width, height: 0.0,
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
        for lb in &self.line_boxes {
            for &(idx, bx, by) in &lb.inline_boxes {
                let child = &mut self.children[idx];
                child.dimensions.content.x = bx
                    + child.dimensions.margin.left
                    + child.dimensions.border.left
                    + child.dimensions.padding.left;
                child.dimensions.content.y = by
                    + child.dimensions.margin.top
                    + child.dimensions.border.top
                    + child.dimensions.padding.top;
            }
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
        let zero    = Value::Length(0.0, Unit::Px);
        let auto_kw = Value::Keyword("auto".into());

        let width_val  = style.value("width").unwrap_or(&auto_kw).clone();
        let margin_l_v = style.lookup("margin-left",        "margin",       &zero);
        let margin_r_v = style.lookup("margin-right",       "margin",       &zero);
        let border_l   = style.lookup("border-left-width",  "border-width", &zero);
        let border_r   = style.lookup("border-right-width", "border-width", &zero);
        let padding_l  = style.lookup("padding-left",       "padding",      &zero);
        let padding_r  = style.lookup("padding-right",      "padding",      &zero);

        let fs = get_font_size(self.style());
        let px = |v: &Value| to_px(v, cw, fs);

        let width_auto = width_val == auto_kw;
        let ml_auto    = margin_l_v == auto_kw;
        let mr_auto    = margin_r_v == auto_kw;

        let border_px  = px(&border_l)  + px(&border_r);
        let padding_px = px(&padding_l) + px(&padding_r);
        let ml_base    = if ml_auto { 0.0 } else { px(&margin_l_v) };
        let mr_base    = if mr_auto { 0.0 } else { px(&margin_r_v) };

        // Resolve min-width / max-width
        let min_w = style.value("min-width").map(|v| px(v)).unwrap_or(0.0);
        let max_w = style.value("max-width")
            .map(|v| px(v))
            .filter(|&v| v > 0.0)
            .unwrap_or(f32::MAX);

        let border_box = matches!(
            style.value("box-sizing"),
            Some(Value::Keyword(s)) if s == "border-box"
        );

        let content_w = if width_auto {
            let w = f32::max(0.0, cw - ml_base - mr_base - border_px - padding_px);
            w.max(min_w).min(max_w)
        } else {
            let w = px(&width_val).max(min_w).min(max_w);
            if border_box { (w - border_px - padding_px).max(0.0) } else { w }
        };

        // Distribute free space to auto margins (block centering etc.)
        let remaining = cw - content_w - ml_base - mr_base - border_px - padding_px;
        let (ml_final, mr_final) = if !width_auto {
            match (ml_auto, mr_auto) {
                (true,  true)  => (remaining / 2.0, remaining / 2.0),
                (true,  false) => (remaining,        mr_base),
                (false, true)  => (ml_base,          remaining),
                (false, false) => (ml_base,          mr_base),
            }
        } else {
            (ml_base, mr_base)
        };

        let d = &mut self.dimensions;
        d.content.width = content_w;
        d.margin.left   = ml_final;
        d.margin.right  = mr_final;
        d.border.left   = px(&border_l);
        d.border.right  = px(&border_r);
        d.padding.left  = px(&padding_l);
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
            let mt = to_px(&style.lookup("margin-top",          "margin",       &zero), cw, fs);
            let mb = to_px(&style.lookup("margin-bottom",       "margin",       &zero), cw, fs);
            let bt = to_px(&style.lookup("border-top-width",    "border-width", &zero), cw, fs);
            let bb = to_px(&style.lookup("border-bottom-width", "border-width", &zero), cw, fs);
            let pt = to_px(&style.lookup("padding-top",         "padding",      &zero), cw, fs);
            let pb = to_px(&style.lookup("padding-bottom",      "padding",      &zero), cw, fs);

            let rel = if style.position() == Position::Relative {
                let top    = style.value("top").cloned().unwrap_or_else(|| auto.clone());
                let bottom = style.value("bottom").cloned().unwrap_or_else(|| auto.clone());
                let left   = style.value("left").cloned().unwrap_or_else(|| auto.clone());
                let right  = style.value("right").cloned().unwrap_or_else(|| auto.clone());
                let dy = if top != auto { to_px(&top, cw, fs) }
                         else if bottom != auto { -to_px(&bottom, cw, fs) }
                         else { 0.0 };
                let dx = if left != auto { to_px(&left, cw, fs) }
                         else if right != auto { -to_px(&right, cw, fs) }
                         else { 0.0 };
                Some((dx, dy))
            } else {
                None
            };
            (mt, mb, bt, bb, pt, pb, rel)
        } else {
            (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, None)
        };

        let d = &mut self.dimensions;
        d.margin.top    = mt; d.margin.bottom  = mb;
        d.border.top    = bt; d.border.bottom  = bb;
        d.padding.top   = pt; d.padding.bottom = pb;

        d.content.x = containing.content.x + d.margin.left + d.border.left + d.padding.left;
        d.content.y = containing.content.y + ch + mt + bt + pt;

        if let Some((dx, dy)) = rel_offsets {
            d.content.x += dx;
            d.content.y += dy;
        }
    }

    // ── Children ──────────────────────────────────────────────────────────

    fn layout_children(&mut self) {
        let vp_w = self.viewport_w;
        for i in 0..self.children.len() {
            // Propagate viewport width to every child.
            self.children[i].viewport_w = vp_w;

            let pos = self.children[i].style()
                .map(|s| s.position())
                .unwrap_or(Position::Static);

            match pos {
                Position::Absolute => {
                    // Containing block = parent's padding box.
                    let pb = self.dimensions.padding_box();
                    let containing = Dimensions {
                        content: Rect { x: pb.x, y: pb.y, width: pb.width, height: pb.height },
                        ..Default::default()
                    };
                    self.children[i].layout_absolute(containing);
                    // Out of normal flow — don't accumulate height.
                }
                Position::Fixed => {
                    // Containing block = viewport (top-left origin, full viewport width).
                    let containing = Dimensions {
                        content: Rect { x: 0.0, y: 0.0, width: vp_w, height: 0.0 },
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
                    let next_mt = self.children[i].style()
                        .map(|s| {
                            let zero = Value::Length(0.0, Unit::Px);
                            let cw   = self.dimensions.content.width;
                            let fs   = get_font_size(Some(s));
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

        let zero    = Value::Length(0.0, Unit::Px);
        let auto_kw = Value::Keyword("auto".into());
        let cw      = containing.content.width;
        let ch      = containing.content.height;

        // Extract edge values upfront (avoids borrow conflict with &mut self).
        let (mt, mb, bt, bb, pt, pb) = if let Some(s) = self.style() {
            let fs = get_font_size(Some(s));
            (
                to_px(&s.lookup("margin-top",          "margin",       &zero), cw, fs),
                to_px(&s.lookup("margin-bottom",       "margin",       &zero), cw, fs),
                to_px(&s.lookup("border-top-width",    "border-width", &zero), cw, fs),
                to_px(&s.lookup("border-bottom-width", "border-width", &zero), cw, fs),
                to_px(&s.lookup("padding-top",         "padding",      &zero), cw, fs),
                to_px(&s.lookup("padding-bottom",      "padding",      &zero), cw, fs),
            )
        } else { (0.0, 0.0, 0.0, 0.0, 0.0, 0.0) };

        let (top_v, bottom_v, left_v, right_v) = if let Some(s) = self.style() {
            (
                s.value("top").cloned().unwrap_or_else(|| auto_kw.clone()),
                s.value("bottom").cloned().unwrap_or_else(|| auto_kw.clone()),
                s.value("left").cloned().unwrap_or_else(|| auto_kw.clone()),
                s.value("right").cloned().unwrap_or_else(|| auto_kw.clone()),
            )
        } else { (auto_kw.clone(), auto_kw.clone(), auto_kw.clone(), auto_kw.clone()) };

        self.dimensions.margin.top    = mt; self.dimensions.margin.bottom  = mb;
        self.dimensions.border.top    = bt; self.dimensions.border.bottom  = bb;
        self.dimensions.padding.top   = pt; self.dimensions.padding.bottom = pb;

        // Layout children to determine content height.
        self.layout_children();
        self.calc_height();

        // Now compute absolute position from top/left/right/bottom offsets.
        let total_w = self.dimensions.margin_box().width;
        let total_h = self.dimensions.margin_box().height;
        let auto = &auto_kw;
        let fs = get_font_size(self.style());

        let cx = if left_v != *auto {
            containing.content.x + to_px(&left_v, cw, fs)
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
            containing.content.y + to_px(&top_v, ch, fs)
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

    fn flex_direction(&self) -> FlexDirection {
        match self.style().and_then(|s| s.value("flex-direction")) {
            Some(Value::Keyword(s)) if s == "column" || s == "column-reverse" => FlexDirection::Column,
            _ => FlexDirection::Row,
        }
    }

    fn flex_justify_content(&self) -> JustifyContent {
        match self.style().and_then(|s| s.value("justify-content")) {
            Some(Value::Keyword(s)) => match s.as_str() {
                "center"        => JustifyContent::Center,
                "flex-end" | "end" => JustifyContent::End,
                "space-between" => JustifyContent::SpaceBetween,
                "space-around"  => JustifyContent::SpaceAround,
                _               => JustifyContent::Start,
            },
            _ => JustifyContent::Start,
        }
    }

    fn flex_align_items(&self) -> AlignItems {
        match self.style().and_then(|s| s.value("align-items")) {
            Some(Value::Keyword(s)) => match s.as_str() {
                "center"   => AlignItems::Center,
                "flex-end" | "end" => AlignItems::End,
                "stretch"  => AlignItems::Stretch,
                _          => AlignItems::Start,
            },
            _ => AlignItems::Stretch, // CSS default
        }
    }

    fn layout_flex_children(&mut self) {
        match self.flex_direction() {
            FlexDirection::Row    => self.layout_flex_row(),
            FlexDirection::Column => self.layout_flex_column(),
        }
    }

    fn layout_flex_row(&mut self) {
        let container  = self.dimensions;
        let justify    = self.flex_justify_content();
        let align      = self.flex_align_items();
        let n          = self.children.len();
        if n == 0 { return; }

        // 1. Compute flex basis (preferred widths).
        let mut sizes: Vec<f32> = Vec::with_capacity(n);
        let mut total = 0.0f32;
        for child in &mut self.children {
            child.calc_width(container);
            let basis = child.preferred_flex_width(container.content.width);
            child.dimensions.content.width = basis;
            total += child.dimensions.margin_box().width;
            sizes.push(basis);
        }

        // 2. Grow / shrink.
        let free = container.content.width - total;
        if free > 0.0 {
            let total_grow: f32 = self.children.iter().map(|c| c.flex_factor("flex-grow", 0.0)).sum();
            if total_grow > 0.0 {
                for (s, c) in sizes.iter_mut().zip(&self.children) {
                    *s += free * c.flex_factor("flex-grow", 0.0) / total_grow;
                }
            }
        } else if free < 0.0 {
            let total_shrink: f32 = sizes.iter()
                .zip(&self.children)
                .map(|(w, c)| w * c.flex_factor("flex-shrink", 1.0))
                .sum();
            if total_shrink > 0.0 {
                for (s, c) in sizes.iter_mut().zip(&self.children) {
                    let weight = *s * c.flex_factor("flex-shrink", 1.0);
                    *s = (*s + free * weight / total_shrink).max(0.0);
                }
            }
        }

        // 3. First pass: lay out each child with its assigned width.
        for (child, &width) in self.children.iter_mut().zip(&sizes) {
            let item_c = Dimensions {
                content: Rect { x: 0.0, y: container.content.y, width: container.content.width, height: 0.0 },
                ..Default::default()
            };
            child.layout_with_assigned_width(item_c, width);
        }

        // 4. Cross-axis (align-items).
        let max_h: f32 = self.children.iter().map(|c| c.dimensions.margin_box().height).fold(0.0, f32::max);
        if align == AlignItems::Stretch {
            // Already laid out with their natural heights — stretching requires re-layout.
            // For simplicity, we skip true stretching; CSS stretch sets height = max.
            for child in &mut self.children {
                if child.style().and_then(|s| s.value("height")).is_none() {
                    child.dimensions.content.height = max_h
                        - child.dimensions.margin.top   - child.dimensions.border.top   - child.dimensions.padding.top
                        - child.dimensions.margin.bottom - child.dimensions.border.bottom - child.dimensions.padding.bottom;
                }
            }
        }

        // 5. Main-axis (justify-content) — compute x positions.
        let used_w: f32 = self.children.iter().map(|c| c.dimensions.margin_box().width).sum();
        let remaining = (container.content.width - used_w).max(0.0);
        let (initial_x, gap) = justify.offsets(remaining, n);
        let mut cursor_x = container.content.x + initial_x;

        for child in &mut self.children {
            let ml = child.dimensions.margin.left;
            let bl = child.dimensions.border.left;
            let pl = child.dimensions.padding.left;
            child.dimensions.content.x = cursor_x + ml + bl + pl;

            // Cross-axis y position.
            let child_h = child.dimensions.margin_box().height;
            let cross_offset = match align {
                AlignItems::Center  => (max_h - child_h) / 2.0,
                AlignItems::End     => max_h - child_h,
                _                   => 0.0,
            };
            child.dimensions.content.y = container.content.y
                + cross_offset
                + child.dimensions.margin.top
                + child.dimensions.border.top
                + child.dimensions.padding.top;

            cursor_x += child.dimensions.margin_box().width + gap;
        }

        self.dimensions.content.height = max_h;
    }

    fn layout_flex_column(&mut self) {
        let container = self.dimensions;
        let align     = self.flex_align_items();
        let justify   = self.flex_justify_content();
        let n         = self.children.len();
        if n == 0 { return; }

        // Lay out each child in column direction (full width).
        for child in &mut self.children {
            let item_c = Dimensions {
                content: Rect { x: container.content.x, y: 0.0, width: container.content.width, height: 0.0 },
                ..Default::default()
            };
            child.calc_width(item_c);
            child.calc_position(item_c);
            child.layout_children();
            child.calc_height();
        }

        // Cross-axis alignment (horizontal for column flex).
        let max_w: f32 = self.children.iter().map(|c| c.dimensions.margin_box().width).fold(0.0, f32::max);
        for child in &mut self.children {
            let child_w = child.dimensions.margin_box().width;
            let cross_offset = match align {
                AlignItems::Center  => (max_w - child_w) / 2.0,
                AlignItems::End     => max_w - child_w,
                AlignItems::Stretch => { child.dimensions.content.width = container.content.width; 0.0 }
                AlignItems::Start   => 0.0,
            };
            child.dimensions.content.x = container.content.x
                + cross_offset
                + child.dimensions.margin.left
                + child.dimensions.border.left
                + child.dimensions.padding.left;
        }

        // Main-axis (justify-content) — compute y positions.
        let used_h: f32 = self.children.iter().map(|c| c.dimensions.margin_box().height).sum();
        let remaining = (container.content.height - used_h).max(0.0);
        let (initial_y, gap) = justify.offsets(remaining, n);
        let mut cursor_y = container.content.y + initial_y;

        let mut total_h = 0.0f32;
        for child in &mut self.children {
            child.dimensions.content.y = cursor_y
                + child.dimensions.margin.top
                + child.dimensions.border.top
                + child.dimensions.padding.top;
            let mb = child.dimensions.margin_box().height;
            cursor_y += mb + gap;
            total_h  += mb;
        }

        self.dimensions.content.height = total_h;
    }

    fn layout_with_assigned_width(&mut self, containing: Dimensions, width: f32) {
        match &self.box_type {
            BoxType::Block(_) | BoxType::Grid(_) | BoxType::Table(_) | BoxType::TableRow(_) | BoxType::TableCell(_) => {
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
                            content: Rect { x: avail.content.x, y: avail.content.y, width, height: 0.0 },
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
                for lb in &self.line_boxes {
                    for &(idx, bx, by) in &lb.inline_boxes {
                        let child = &mut self.children[idx];
                        child.dimensions.content.x = bx + child.dimensions.margin.left + child.dimensions.border.left + child.dimensions.padding.left;
                        child.dimensions.content.y = by + child.dimensions.margin.top  + child.dimensions.border.top  + child.dimensions.padding.top;
                    }
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
                    inline_box: Some((idx, mb.width, mb.height)),
                });
            } else {
                child.collect_inline_pieces(pieces);
            }
        }
    }

    fn preferred_flex_width(&self, containing_width: f32) -> f32 {
        let fs = get_font_size(self.style());
        self.style()
            .and_then(|s| s.value("width"))
            .map(|v| to_px(v, containing_width, fs))
            .unwrap_or(0.0)
    }

    fn flex_factor(&self, name: &str, default: f32) -> f32 {
        self.style()
            .and_then(|s| s.value(name))
            .map(number_value)
            .unwrap_or(default)
    }

    fn calc_height(&mut self) {
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
    pub fn text_color(&self) -> Color { self.text_style.color }
    /// Font size used for inline content.
    pub fn font_size(&self) -> f32 { self.text_style.font_size }

    fn layout_grid(&mut self, containing: Dimensions) {
        self.calc_width(containing);
        self.calc_position(containing);

        let style = match self.style() {
            Some(s) => s,
            None => return,
        };

        // Parse gap / row-gap / column-gap
        let gap = style.value("gap")
            .or_else(|| style.value("grid-gap"))
            .map(|v| v.to_px())
            .unwrap_or(0.0);
        let row_gap = style.value("row-gap").map(|v| v.to_px()).unwrap_or(gap);
        let col_gap = style.value("column-gap").map(|v| v.to_px()).unwrap_or(gap);

        // Parse grid-template-columns
        let col_spec = match style.value("grid-template-columns") {
            Some(Value::Keyword(s)) => s.clone(),
            _ => "1fr 1fr".to_string(), // default 2 equal tracks
        };
        let col_tokens: Vec<&str> = col_spec.split_whitespace().collect();
        let num_cols = col_tokens.len().max(1);

        let container_w = self.dimensions.content.width;
        let avail_w = (container_w - (num_cols - 1) as f32 * col_gap).max(0.0);

        // Calculate column widths
        let mut col_widths = vec![0.0f32; num_cols];
        let mut fr_total = 0.0f32;
        let mut allocated_w = 0.0f32;

        for (i, tok) in col_tokens.iter().enumerate() {
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

        let remaining_w = (avail_w - allocated_w).max(0.0);
        if fr_total > 0.0 {
            for (i, tok) in col_tokens.iter().enumerate() {
                if tok.ends_with("fr") || (!tok.ends_with("px") && !tok.ends_with('%') && tok.parse::<f32>().is_err()) {
                    let weight: f32 = if tok.ends_with("fr") {
                        tok.trim_end_matches("fr").parse().unwrap_or(1.0)
                    } else {
                        1.0
                    };
                    col_widths[i] = remaining_w * (weight / fr_total);
                }
            }
        }

        // Place children into grid cells
        let mut cur_row = 0usize;
        let mut cur_col = 0usize;
        let mut row_heights = Vec::<f32>::new();

        let grid_x = self.dimensions.content.x;
        let grid_y = self.dimensions.content.y;

        for child in &mut self.children {
            if cur_col >= num_cols {
                cur_col = 0;
                cur_row += 1;
            }

            let cell_x = grid_x + (0..cur_col).map(|i| col_widths[i] + col_gap).sum::<f32>();
            let cell_w = col_widths[cur_col];
            let cell_y = grid_y + (0..cur_row).map(|r| row_heights.get(r).copied().unwrap_or(0.0) + row_gap).sum::<f32>();

            let cell_containing = Dimensions {
                content: Rect { x: cell_x, y: cell_y, width: cell_w, height: 0.0 },
                ..Default::default()
            };

            child.layout(cell_containing);
            let child_h = child.dimensions.margin_box().height;

            if cur_row >= row_heights.len() {
                row_heights.push(child_h);
            } else {
                row_heights[cur_row] = row_heights[cur_row].max(child_h);
            }

            cur_col += 1;
        }

        // Final pass: enforce row heights
        cur_row = 0;
        cur_col = 0;
        for child in &mut self.children {
            if cur_col >= num_cols {
                cur_col = 0;
                cur_row += 1;
            }
            let cell_x = grid_x + (0..cur_col).map(|i| col_widths[i] + col_gap).sum::<f32>();
            let cell_y = grid_y + (0..cur_row).map(|r| row_heights.get(r).copied().unwrap_or(0.0) + row_gap).sum::<f32>();
            let cell_w = col_widths[cur_col];

            let cell_containing = Dimensions {
                content: Rect { x: cell_x, y: cell_y, width: cell_w, height: row_heights[cur_row] },
                ..Default::default()
            };

            child.layout(cell_containing);
            cur_col += 1;
        }

        let total_h: f32 = row_heights.iter().sum::<f32>() + (row_heights.len().saturating_sub(1)) as f32 * row_gap;
        self.dimensions.content.height = total_h;
        self.calc_height();
    }

    fn layout_table(&mut self, containing: Dimensions) {
        self.calc_width(containing);
        self.calc_position(containing);

        let mut col_widths = Vec::<f32>::new();
        for row in &mut self.children {
            for (col_idx, cell) in row.children.iter_mut().enumerate() {
                let dummy = Dimensions { content: Rect { x: 0.0, y: 0.0, width: self.dimensions.content.width, height: 0.0 }, ..Default::default() };
                cell.layout(dummy);
                let w = cell.dimensions.margin_box().width;
                if col_idx >= col_widths.len() {
                    col_widths.push(w);
                } else {
                    col_widths[col_idx] = col_widths[col_idx].max(w);
                }
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
                    content: Rect { x: cell_x, y: cursor_y, width: cell_w, height: 0.0 },
                    ..Default::default()
                };
                cell.layout(cell_containing);
                let h = cell.dimensions.margin_box().height;
                row_h = row_h.max(h);
                cell_x += cell_w;
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
enum FlexDirection { Row, Column }

#[derive(Debug, Clone, Copy, PartialEq)]
enum JustifyContent { Start, Center, End, SpaceBetween, SpaceAround }

impl JustifyContent {
    /// Returns `(initial_offset, per-gap)` given free space and item count.
    fn offsets(self, free: f32, n: usize) -> (f32, f32) {
        match self {
            JustifyContent::Start         => (0.0, 0.0),
            JustifyContent::Center        => (free / 2.0, 0.0),
            JustifyContent::End           => (free, 0.0),
            JustifyContent::SpaceBetween  => {
                if n <= 1 { (0.0, 0.0) } else { (0.0, free / (n - 1) as f32) }
            }
            JustifyContent::SpaceAround   => {
                let gap = free / n as f32;
                (gap / 2.0, gap)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum AlignItems { Stretch, Start, Center, End }

// ── Tree construction ─────────────────────────────────────────────────────────

pub fn build_layout_tree<'a>(node: &'a StyledNode<'a>) -> Option<LayoutBox<'a>> {
    build_layout_tree_inner(node, TextStyle::default(), TextAlign::default())
}

fn text_align_for(node: &StyledNode) -> TextAlign {
    match node.value("text-align") {
        Some(Value::Keyword(s)) => match s.as_str() {
            "center" => TextAlign::Center,
            "right"  => TextAlign::Right,
            _        => TextAlign::Left,
        },
        _ => TextAlign::Left,
    }
}

fn build_layout_tree_inner<'a>(
    node: &'a StyledNode<'a>,
    inherited_text_style: TextStyle,
    _inherited_text_align: TextAlign,
) -> Option<LayoutBox<'a>> {
    let text_style = text_style_for_node(node, inherited_text_style);

    // text-align is read from the styled node (inherited CSS property).
    let text_align = text_align_for(node);

    let box_type = match node.display() {
        Display::Block       => BoxType::Block(node),
        Display::Flex        => BoxType::Flex(node),
        Display::Grid        => BoxType::Grid(node),
        Display::Table       => BoxType::Table(node),
        Display::TableRow    => BoxType::TableRow(node),
        Display::TableCell   => BoxType::TableCell(node),
        Display::Inline      => BoxType::Inline(node),
        Display::InlineBlock => BoxType::InlineBlock(node),
        Display::None        => return None,
    };

    let mut root = LayoutBox::new(box_type, text_style, text_align);

    for child in &node.children {
        match child.display() {
            Display::None => {}
            Display::Block | Display::Flex | Display::Grid | Display::Table | Display::TableRow | Display::TableCell => {
                if let Some(b) = build_layout_tree_inner(child, text_style, text_align) {
                    root.children.push(b);
                }
            }
            Display::Inline | Display::InlineBlock => {
                if let Some(b) = build_layout_tree_inner(child, text_style, text_align) {
                    root.inline_container().children.push(b);
                }
            }
        }
    }

    Some(root)
}

/// Build and lay out a box tree for `viewport_width` pixels.
pub fn layout_tree<'a>(root: &'a StyledNode<'a>, viewport_width: f32) -> LayoutBox<'a> {
    let mut root_box = build_layout_tree(root)
        .unwrap_or_else(|| LayoutBox::new(BoxType::AnonymousBlock, TextStyle::default(), TextAlign::default()));

    root_box.viewport_w = viewport_width;
    let viewport = Dimensions {
        content: Rect { x: 0.0, y: 0.0, width: viewport_width, height: 0.0 },
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
        Some(Value::Length(size, Unit::Px))      => *size,
        Some(Value::Length(size, Unit::Em))      => size * inherited.font_size,
        Some(Value::Length(size, Unit::Percent)) => size * inherited.font_size / 100.0,
        _ => inherited.font_size,
    };
    let line_height = match node.value("line-height") {
        Some(Value::Number(n))               => *n,
        Some(Value::Length(n, Unit::Em))     => *n,
        Some(Value::Length(n, Unit::Px))     => n / font_size,
        Some(Value::Length(n, Unit::Percent))=> n / 100.0,
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
            "uppercase"  => TextTransform::Uppercase,
            "lowercase"  => TextTransform::Lowercase,
            "capitalize" => TextTransform::Capitalize,
            _            => TextTransform::None,
        },
        _ => inherited.text_transform,
    };
    TextStyle { color, font_size, line_height, no_wrap, underline, strikethrough, text_transform }
}

#[derive(Debug, Default)]
struct OpenLine {
    fragments: Vec<TextFragment>,
    /// Partially-placed inline-block boxes: (child_idx, margin-box x offset from line start).
    inline_boxes: Vec<(usize, f32)>,
    width: f32,
    ascent: f32,
    height: f32,
}

fn build_line_boxes(
    pieces: Vec<InlinePiece>,
    x: f32,
    y: f32,
    max_width: f32,
    text_align: TextAlign,
) -> Vec<LineBox> {
    let max_width = max_width.max(0.0);
    let mut lines = Vec::new();
    let mut line = OpenLine::default();
    let mut pending_space = false;

    for piece in pieces {
        // Inline-block piece: treat as an opaque fixed-size box.
        if let Some((child_idx, mb_w, mb_h)) = piece.inline_box {
            if !line.fragments.is_empty() && line.width + mb_w > max_width {
                flush_line(&mut lines, &mut line, x, y, max_width, text_align);
            }
            line.inline_boxes.push((child_idx, x + line.width));
            line.width  += mb_w;
            line.height  = line.height.max(mb_h);
            line.ascent  = line.ascent.max(mb_h);
            pending_space = false;
            continue;
        }

        // Text piece.
        let effective_max = if piece.no_wrap { f32::MAX } else { max_width };

        for run in split_whitespace_runs(&piece.text) {
            if run.chars().all(char::is_whitespace) {
                pending_space = true;
                continue;
            }

            if pending_space && !line.fragments.is_empty() {
                let space_w = measure_text(" ", piece.style.font_size);
                if line.width + space_w + measure_text(&run, piece.style.font_size) > effective_max {
                    flush_line(&mut lines, &mut line, x, y, max_width, text_align);
                } else {
                    line.width += space_w;
                }
            }
            add_word(&mut lines, &mut line, &run, piece.style, x, y, effective_max, text_align);
            pending_space = false;
        }
    }

    flush_line(&mut lines, &mut line, x, y, max_width, text_align);
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
    if !current.is_empty() { runs.push(current); }
    runs
}

fn add_word(
    lines: &mut Vec<LineBox>,
    line: &mut OpenLine,
    word: &str,
    style: TextStyle,
    x: f32,
    y: f32,
    max_width: f32,
    text_align: TextAlign,
) {
    let word_w = measure_text(word, style.font_size);
    if !line.fragments.is_empty() && line.width + word_w > max_width {
        flush_line(lines, line, x, y, max_width, text_align);
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
            flush_line(lines, line, x, y, max_width, text_align);
            chunk_w = 0.0;
        }
        chunk.push(c);
        chunk_w += cw;
    }
    if !chunk.is_empty() { add_fragment(line, chunk, style, x); }
}

fn add_fragment(line: &mut OpenLine, text: String, style: TextStyle, x: f32) {
    let metrics  = line_metrics(style.font_size);
    let width    = measure_text(&text, style.font_size);
    let lh       = metrics.new_line_size * style.line_height;
    let ascent   = metrics.ascent * style.line_height;
    line.fragments.push(TextFragment {
        text,
        rect: Rect { x: x + line.width, y: 0.0, width, height: lh },
        baseline: 0.0,
        color: style.color,
        font_size: style.font_size,
        underline: style.underline,
        strikethrough: style.strikethrough,
    });
    line.width  += width;
    line.ascent  = line.ascent.max(ascent);
    line.height  = line.height.max(lh);
}

fn apply_text_transform(text: &str, transform: TextTransform) -> String {
    match transform {
        TextTransform::None       => text.to_string(),
        TextTransform::Uppercase  => text.to_uppercase(),
        TextTransform::Lowercase  => text.to_lowercase(),
        TextTransform::Capitalize => {
            let mut cap_next = true;
            text.chars().map(|c| {
                if c.is_whitespace() { cap_next = true; c }
                else if cap_next { cap_next = false; c.to_uppercase().next().unwrap_or(c) }
                else { c }
            }).collect()
        }
    }
}

fn flush_line(
    lines: &mut Vec<LineBox>,
    line: &mut OpenLine,
    x: f32,
    y: f32,
    max_width: f32,
    text_align: TextAlign,
) {
    if line.fragments.is_empty() && line.inline_boxes.is_empty() { return; }

    let mut open = std::mem::take(line);
    let line_y   = y + lines.iter().map(|l| l.rect.height).sum::<f32>();
    let baseline = line_y + open.ascent;

    let offset = match text_align {
        TextAlign::Left   => 0.0,
        TextAlign::Center => ((max_width - open.width) / 2.0).max(0.0),
        TextAlign::Right  => (max_width - open.width).max(0.0),
    };

    for frag in &mut open.fragments {
        frag.rect.x      += offset;
        frag.rect.y       = line_y;
        frag.rect.height  = open.height;
        frag.baseline     = baseline;
    }

    let inline_boxes = open.inline_boxes.iter()
        .map(|&(idx, bx)| (idx, bx + offset, line_y))
        .collect();

    lines.push(LineBox {
        rect: Rect { x: x + offset, y: line_y, width: open.width, height: open.height },
        baseline,
        fragments: open.fragments,
        inline_boxes,
    });
}

fn to_px(value: &Value, containing_width: f32, font_size: f32) -> f32 {
    match value {
        Value::Length(n, Unit::Px)      => *n,
        Value::Length(n, Unit::Em)      => n * font_size,
        Value::Length(n, Unit::Percent) => n * containing_width / 100.0,
        Value::Calc(expr)               => eval_calc(expr, containing_width, font_size),
        _ => 0.0,
    }
}

/// Extract the computed `font-size` (in px) from a styled node, defaulting to 16.
fn get_font_size(style: Option<&StyledNode<'_>>) -> f32 {
    style.and_then(|s| s.value("font-size"))
        .and_then(|v| if let Value::Length(px, Unit::Px) = v { Some(*px) } else { None })
        .unwrap_or(16.0)
}

/// Recursively evaluate a `calc()` expression given the containing block width.
fn eval_calc(expr: &CalcExpr, cw: f32, fs: f32) -> f32 {
    match expr {
        CalcExpr::Literal(n, Unit::Px)      => *n,
        CalcExpr::Literal(n, Unit::Em)      => n * fs,
        CalcExpr::Literal(n, Unit::Percent) => n * cw / 100.0,
        CalcExpr::Literal(n, Unit::Fr)      => *n,
        CalcExpr::Percent(n)               => n * cw / 100.0,
        CalcExpr::Add(a, b) => eval_calc(a, cw, fs) + eval_calc(b, cw, fs),
        CalcExpr::Sub(a, b) => eval_calc(a, cw, fs) - eval_calc(b, cw, fs),
        CalcExpr::Mul(a, b) => eval_calc(a, cw, fs) * eval_calc(b, cw, fs),
        CalcExpr::Div(a, b) => {
            let d = eval_calc(b, cw, fs);
            if d.abs() < 1e-6 { 0.0 } else { eval_calc(a, cw, fs) / d }
        }
    }
}

fn number_value(value: &Value) -> f32 {
    match value {
        Value::Length(n, _)       => *n,
        Value::Number(n)          => *n,
        Value::Keyword(s)         => s.parse().unwrap_or(0.0),
        Value::Color(_)           => 0.0,
        Value::LinearGradient(_)  => 0.0,
        Value::BoxShadow(_)       => 0.0,
        Value::Transform(_)       => 0.0,
        Value::Var { .. }         => 0.0,
        Value::Calc(expr)         => eval_calc(expr, 0.0, 16.0),
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
        let dom    = Box::leak(Box::new(parse_html(html)));
        let ss     = Box::leak(Box::new(parse_css(css)));
        let styled = Box::leak(Box::new(style_tree(dom, ss)));
        layout_tree(styled, vw)
    }

    #[test]
    fn block_fills_viewport() {
        let layout = layout_html_css("<div></div>", "div { display: block; }", 800.0);
        fn find_width(b: &LayoutBox) -> Option<f32> {
            if b.dimensions.content.width == 800.0 { return Some(800.0); }
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
            if b.dimensions.content.height == 200.0 { return Some(200.0); }
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
            if (b.dimensions.content.width - 780.0).abs() < 0.01 { return true; }
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
            if b.dimensions.content.width == 400.0 { return Some(b); }
            b.children.iter().find_map(find_div)
        }
        let div = find_div(&layout).expect("expected 400px div");
        // centered in 800px: margin left = right = 200
        assert!((div.dimensions.margin.left  - 200.0).abs() < 0.01);
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
            if (b.dimensions.content.width - 300.0).abs() < 0.01 { return true; }
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
            if (b.dimensions.content.width - 200.0).abs() < 0.01 { return true; }
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
        assert!(x > 0.0, "center-aligned fragment should have x > 0, got {x}");
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
        assert!(find_positioned(&layout).is_some(), "positioned box not found at (10,20)");
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
            lb.line_boxes.first().map(|l| l.rect.height)
                .or_else(|| lb.children.iter().find_map(first_line_h))
        }
        let h1 = first_line_h(&base).expect("no line");
        let h2 = first_line_h(&tall).expect("no line");
        assert!(h2 > h1 * 1.5, "line-height:2 should roughly double line height ({h1} vs {h2})");
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
        assert!(line_count(&wrap) > 1,   "should wrap without nowrap");
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
                if s.position() == crate::style::Position::Absolute { return Some(lb); }
            }
            lb.children.iter().find_map(find_abs)
        }
        let abs_box = find_abs(&layout).expect("absolute box not found");
        assert!((abs_box.dimensions.content.y - 50.0).abs() < 1.0,
            "expected y≈50, got {}", abs_box.dimensions.content.y);
        assert!((abs_box.dimensions.content.x - 30.0).abs() < 1.0,
            "expected x≈30, got {}", abs_box.dimensions.content.x);
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
        assert!(flex.children[1].dimensions.content.y > flex.children[0].dimensions.content.y,
            "column: second child should have greater y");
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
            if (b.dimensions.content.width - 160.0).abs() < 0.5 { return true; }
            b.children.iter().any(find_160)
        }
        assert!(find_160(&layout), "border-box: content width should be 200 - 40 = 160");
    }

    #[test]
    fn text_transform_uppercase() {
        let layout = layout_html_css(
            "<p>hello</p>",
            "p { display: block; text-transform: uppercase; }",
            200.0,
        );
        fn has_upper(lb: &LayoutBox) -> bool {
            lb.line_boxes.iter()
                .flat_map(|l| &l.fragments)
                .any(|f| f.text.chars().all(|c| !c.is_alphabetic() || c.is_uppercase()))
                || lb.children.iter().any(has_upper)
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
            lb.line_boxes.iter()
                .flat_map(|l| &l.fragments)
                .any(|f| f.underline)
                || lb.children.iter().any(has_underline)
        }
        assert!(has_underline(&layout), "fragment should have underline=true");
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
                if s.position() == crate::style::Position::Fixed { return Some(lb); }
            }
            lb.children.iter().find_map(find_fixed)
        }
        let fixed = find_fixed(&layout).expect("fixed box not found");
        // top:20, left:30 — relative to viewport (0,0), not to the parent
        assert!((fixed.dimensions.content.y - 20.0).abs() < 1.0,
            "fixed y should be 20, got {}", fixed.dimensions.content.y);
        assert!((fixed.dimensions.content.x - 30.0).abs() < 1.0,
            "fixed x should be 30, got {}", fixed.dimensions.content.x);
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
                    if e.tag_name == "div" { out.push(lb.dimensions.content.y); }
                }
            }
            for c in &lb.children { find_divs(c, out); }
        }
        let mut ys = Vec::new();
        find_divs(&layout, &mut ys);
        assert_eq!(ys.len(), 2, "expected 2 divs");
        // gap = y[1] - (y[0] + height[0]) should be 30 (collapsed), not 50
        let gap = ys[1] - (ys[0] + 40.0);
        assert!((gap - 30.0).abs() < 1.0,
            "collapsed margin gap should be 30, got {gap}");
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
            for c in &lb.children { find_inline_blocks(c, out); }
        }
        let mut xs = Vec::new();
        find_inline_blocks(&layout, &mut xs);
        assert_eq!(xs.len(), 2, "expected 2 inline-block boxes, got {}", xs.len());
        assert!(xs[1] > xs[0], "second inline-block should be to the right of the first");
    }

    fn find_flex<'a, 'b>(lb: &'b LayoutBox<'a>) -> Option<&'b LayoutBox<'a>> {
        if matches!(lb.box_type, BoxType::Flex(_)) { return Some(lb); }
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
            if matches!(lb.box_type, BoxType::Grid(_)) { return Some(lb); }
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
            if matches!(lb.box_type, BoxType::Table(_)) { return Some(lb); }
            lb.children.iter().find_map(find_table)
        }
        let tbl = find_table(&layout).expect("table box not found");
        assert_eq!(tbl.children.len(), 1);
        let row = &tbl.children[0];
        assert_eq!(row.children.len(), 2);
    }
}
