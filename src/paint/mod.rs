// ============================================================
//  paint/mod.rs  —  Painter: layout → pixel canvas → PPM
// ============================================================
//
//  Traverses the layout tree to produce a flat display list
//  (filled rectangles and borders), then rasterises it onto a
//  simple RGB canvas.  The result can be written as a PPM P6
//  binary image — viewable in most image viewers.
//
//  What gets painted:
//   • background-color on any box that has one
//   • border (all four sides) when border-width + border-color are set

use std::rc::Rc;

use crate::css::parser::{Color, ColorStop, ConicGradient, LinearGradient, RadialGradient, Unit, Value};
use crate::dom::NodeType;
use crate::image::RasterImage;
use crate::layout::{BoxType, LayoutBox, ObjectFit, Rect, TextFragment};
use crate::style::{Position, StyledNode};
use crate::text::{line_metrics, measure_text, rasterize};

// ── Display list ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum DisplayCommand {
    SolidColor(Color, Rect),
    /// Rectangle with uniform corner radius (pixels).
    RoundedRect(Color, Rect, f32),
    Text(TextFragment),
    /// Push an axis-aligned clip rectangle (intersected with the current one).
    PushClip(Rect),
    /// Restore the previous clip rectangle.
    PopClip,
    /// Linear gradient background: (gradient spec, rect, opacity).
    LinearGradient(LinearGradient, Rect, f32),
    /// Radial gradient background: (gradient spec, rect, opacity).
    RadialGradient(RadialGradient, Rect, f32),
    /// Conic gradient background: (gradient spec, rect, opacity).
    ConicGradient(ConicGradient, Rect, f32),
    /// A decoded bitmap: `source` (in image pixels) is scaled onto `dest`.
    Image {
        image: Rc<RasterImage>,
        dest: Rect,
        source: Rect,
        opacity: f32,
    },
    /// Drop shadow around box: (rect, offset_x, offset_y, blur, color, opacity).
    BoxShadow {
        rect: Rect,
        offset_x: f32,
        offset_y: f32,
        blur: f32,
        color: Color,
        opacity: f32,
    },
}

impl DisplayCommand {
    pub fn offset(&mut self, dx: f32, dy: f32) {
        match self {
            DisplayCommand::SolidColor(_, rect) => {
                rect.x += dx;
                rect.y += dy;
            }
            DisplayCommand::RoundedRect(_, rect, _) => {
                rect.x += dx;
                rect.y += dy;
            }
            DisplayCommand::Text(frag) => {
                frag.rect.x += dx;
                frag.rect.y += dy;
            }
            DisplayCommand::PushClip(rect) => {
                rect.x += dx;
                rect.y += dy;
            }
            DisplayCommand::PopClip => {}
            DisplayCommand::LinearGradient(_, rect, _) => {
                rect.x += dx;
                rect.y += dy;
            }
            DisplayCommand::RadialGradient(_, rect, _) => {
                rect.x += dx;
                rect.y += dy;
            }
            DisplayCommand::ConicGradient(_, rect, _) => {
                rect.x += dx;
                rect.y += dy;
            }
            DisplayCommand::Image { dest, .. } => {
                dest.x += dx;
                dest.y += dy;
            }
            DisplayCommand::BoxShadow { rect, .. } => {
                rect.x += dx;
                rect.y += dy;
            }
        }
    }
}

pub type DisplayList = Vec<DisplayCommand>;

pub fn build_display_list(layout_root: &LayoutBox) -> DisplayList {
    let mut list = Vec::new();
    render_stacking_context(&mut list, layout_root);
    list
}

enum PositionedLayer<'layout, 'style> {
    Auto(&'layout LayoutBox<'style>),
    Context(&'layout LayoutBox<'style>),
}

#[derive(Default)]
struct StackingLayers<'layout, 'style> {
    negative: Vec<(i32, usize, &'layout LayoutBox<'style>)>,
    zero: Vec<(usize, PositionedLayer<'layout, 'style>)>,
    positive: Vec<(i32, usize, &'layout LayoutBox<'style>)>,
}

// Paint a simplified CSS stacking context: root decorations, negative contexts,
// normal flow, positioned auto/zero layers, then positive contexts.
fn render_stacking_context(list: &mut DisplayList, root: &LayoutBox) {
    let opacity = node_opacity(root);
    render_stacking_context_with_opacity(list, root, opacity);
}

fn node_opacity(lb: &LayoutBox) -> f32 {
    styled_node(lb)
        .and_then(|s| s.value("opacity"))
        .map(|v| match v {
            Value::Number(n) => n.clamp(0.0, 1.0),
            Value::Length(n, _) => n.clamp(0.0, 1.0),
            _ => 1.0,
        })
        .unwrap_or(1.0)
}

fn apply_opacity(color: Color, opacity: f32) -> Color {
    Color {
        r: color.r,
        g: color.g,
        b: color.b,
        a: (color.a as f32 * opacity) as u8,
    }
}

fn render_stacking_context_with_opacity(list: &mut DisplayList, root: &LayoutBox, opacity: f32) {
    let mut layers = StackingLayers::default();
    let mut source_order = 0;
    collect_stacking_layers(root, false, &mut source_order, &mut layers);

    layers
        .negative
        .sort_by_key(|(z_index, order, _)| (*z_index, *order));
    layers.zero.sort_by_key(|(order, _)| *order);
    layers
        .positive
        .sort_by_key(|(z_index, order, _)| (*z_index, *order));

    render_box_decorations_with_opacity(list, root, opacity);

    for (_, _, context) in layers.negative {
        let child_opacity = opacity * node_opacity(context);
        render_stacking_context_with_opacity(list, context, child_opacity);
    }

    render_in_flow_descendants_with_opacity(list, root, opacity);
    render_text_with_opacity(list, root, opacity);

    for (_, layer) in layers.zero {
        match layer {
            PositionedLayer::Auto(lb) => {
                render_subtree_without_contexts_with_opacity(list, lb, opacity * node_opacity(lb))
            }
            PositionedLayer::Context(lb) => {
                let child_opacity = opacity * node_opacity(lb);
                render_stacking_context_with_opacity(list, lb, child_opacity);
            }
        }
    }

    for (_, _, context) in layers.positive {
        let child_opacity = opacity * node_opacity(context);
        render_stacking_context_with_opacity(list, context, child_opacity);
    }
}

fn collect_stacking_layers<'layout, 'style>(
    root: &'layout LayoutBox<'style>,
    inside_positioned_subtree: bool,
    source_order: &mut usize,
    layers: &mut StackingLayers<'layout, 'style>,
) {
    for child in &root.children {
        let order = *source_order;
        *source_order += 1;

        if establishes_stacking_context(child) {
            let z_index = styled_node(child)
                .and_then(StyledNode::z_index)
                .unwrap_or(0);
            match z_index.cmp(&0) {
                std::cmp::Ordering::Less => layers.negative.push((z_index, order, child)),
                std::cmp::Ordering::Equal => {
                    layers.zero.push((order, PositionedLayer::Context(child)));
                }
                std::cmp::Ordering::Greater => layers.positive.push((z_index, order, child)),
            }
            continue;
        }

        let is_positioned = is_positioned(child);
        if is_positioned && !inside_positioned_subtree {
            layers.zero.push((order, PositionedLayer::Auto(child)));
        }
        collect_stacking_layers(
            child,
            inside_positioned_subtree || is_positioned,
            source_order,
            layers,
        );
    }
}

fn render_in_flow_descendants_with_opacity(list: &mut DisplayList, root: &LayoutBox, opacity: f32) {
    for child in &root.children {
        render_in_flow_subtree_with_opacity(list, child, opacity * node_opacity(child));
    }
}

fn overflow_hidden(lb: &LayoutBox) -> bool {
    styled_node(lb)
        .and_then(|s| s.value("overflow"))
        .map(|v| matches!(v, Value::Keyword(s) if s == "hidden"))
        .unwrap_or(false)
}

fn render_in_flow_subtree_with_opacity(list: &mut DisplayList, lb: &LayoutBox, opacity: f32) {
    if establishes_stacking_context(lb) || is_positioned(lb) {
        return;
    }
    render_box_decorations_with_opacity(list, lb, opacity);
    render_list_marker(list, lb, opacity);

    let clip = overflow_hidden(lb);
    if clip {
        list.push(DisplayCommand::PushClip(lb.dimensions.padding_box()));
    }

    for child in &lb.children {
        render_in_flow_subtree_with_opacity(list, child, opacity * node_opacity(child));
    }
    render_text_with_opacity(list, lb, opacity);

    if clip {
        list.push(DisplayCommand::PopClip);
    }
}

/// Renders the bullet/marker for `<li>` elements.
fn render_list_marker(list: &mut DisplayList, lb: &LayoutBox, opacity: f32) {
    let style = match styled_node(lb) {
        Some(s) => s,
        None => return,
    };
    let NodeType::Element(elem) = &style.node.node_type else {
        return;
    };
    if elem.tag_name != "li" {
        return;
    }

    let marker = match style.value("list-style-type") {
        Some(Value::Keyword(s)) => match s.as_str() {
            "disc" => "•",
            "circle" => "◦",
            "square" => "▪",
            "none" => return,
            _ => "•",
        },
        None => "•", // default when inheriting from <ul>
        _ => return,
    };

    let font_size = lb.font_size();
    let color = apply_opacity(lb.text_color(), opacity);
    // Use the first line-box baseline when available, otherwise estimate.
    let baseline = lb
        .line_boxes
        .first()
        .map(|l| l.baseline)
        .or_else(|| {
            lb.children
                .iter()
                .flat_map(|c| c.line_boxes.iter())
                .next()
                .map(|l| l.baseline)
        })
        .unwrap_or_else(|| {
            let m = line_metrics(font_size);
            lb.dimensions.content.y + m.ascent
        });

    let frag = TextFragment {
        text: marker.to_string(),
        rect: Rect {
            x: lb.dimensions.content.x - font_size * 1.1,
            y: baseline - font_size,
            width: font_size,
            height: font_size * 1.2,
        },
        baseline,
        color,
        font_size,
        underline: false,
        strikethrough: false,
    };
    list.push(DisplayCommand::Text(frag));
}

fn render_subtree_without_contexts_with_opacity(
    list: &mut DisplayList,
    lb: &LayoutBox,
    opacity: f32,
) {
    if establishes_stacking_context(lb) {
        return;
    }
    render_box_decorations_with_opacity(list, lb, opacity);
    render_list_marker(list, lb, opacity);

    let clip = overflow_hidden(lb);
    if clip {
        list.push(DisplayCommand::PushClip(lb.dimensions.padding_box()));
    }

    for child in &lb.children {
        render_subtree_without_contexts_with_opacity(list, child, opacity * node_opacity(child));
    }
    render_text_with_opacity(list, lb, opacity);

    if clip {
        list.push(DisplayCommand::PopClip);
    }
}

fn render_box_decorations_with_opacity(list: &mut DisplayList, lb: &LayoutBox, opacity: f32) {
    render_background_with_opacity(list, lb, opacity);
    render_image_with_opacity(list, lb, opacity);
    render_form_control(list, lb, opacity);
    render_borders_with_opacity(list, lb, opacity);
}

// ── Form controls ─────────────────────────────────────────────────────────────

/// Colours for the built-in control widgets.
const CONTROL_BACKGROUND: Color = Color {
    r: 255,
    g: 255,
    b: 255,
    a: 255,
};
const CONTROL_BORDER: Color = Color {
    r: 154,
    g: 168,
    b: 184,
    a: 255,
};
const CONTROL_FOCUS: Color = Color {
    r: 52,
    g: 130,
    b: 214,
    a: 255,
};
const CONTROL_DISABLED: Color = Color {
    r: 236,
    g: 239,
    b: 242,
    a: 255,
};
const CONTROL_TEXT: Color = Color {
    r: 33,
    g: 43,
    b: 54,
    a: 255,
};
const PLACEHOLDER_TEXT: Color = Color {
    r: 150,
    g: 160,
    b: 172,
    a: 255,
};

/// Paint the widget for `<input>` and `<textarea>`.
///
/// The UA look is drawn here rather than in the DOM so that author CSS keeps
/// working: backgrounds and borders from the cascade are painted first, and
/// this only fills in what a stylesheet cannot express — the value text, the
/// placeholder, the caret and the check mark.
fn render_form_control(list: &mut DisplayList, lb: &LayoutBox, opacity: f32) {
    let Some(styled) = styled_node(lb) else {
        return;
    };
    let NodeType::Element(element) = &styled.node.node_type else {
        return;
    };
    if !matches!(element.tag_name.as_str(), "input" | "textarea") {
        return;
    }

    let content = lb.dimensions.content;
    let padding_box = lb.dimensions.padding_box();
    let focused = lb.is_focused();
    let disabled = element.is_disabled();

    if element.is_checkable() {
        render_checkable(list, element, padding_box, focused, opacity);
        return;
    }
    if element.tag_name == "input" && element.input_type() == "hidden" {
        return;
    }

    // Field background and frame.
    let background = if disabled {
        CONTROL_DISABLED
    } else {
        CONTROL_BACKGROUND
    };
    let radius = get_border_radius(lb).max(3.0);
    let frame = if focused {
        CONTROL_FOCUS
    } else {
        CONTROL_BORDER
    };
    render_framed_box(
        list,
        padding_box,
        radius,
        apply_opacity(background, opacity),
        apply_opacity(frame, opacity),
        if focused { 2.0 } else { 1.0 },
    );

    // Value, or placeholder when empty.
    let font_size = lb.font_size();
    let metrics = line_metrics(font_size);
    let value = element.control_value();
    let showing_placeholder = value.is_empty();
    let text = if showing_placeholder {
        element.get_attr("placeholder").unwrap_or("").to_string()
    } else {
        value.clone()
    };
    // Placeholder text and a disabled value are both drawn muted.
    let text_color = if showing_placeholder || disabled {
        PLACEHOLDER_TEXT
    } else {
        CONTROL_TEXT
    };

    list.push(DisplayCommand::PushClip(padding_box));
    let lines: Vec<String> = if element.tag_name == "textarea" {
        text.split('\n').map(str::to_string).collect()
    } else {
        // A single-line field shows newlines as spaces rather than wrapping.
        vec![text.replace('\n', " ")]
    };
    for (index, line) in lines.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        let top = content.y + index as f32 * metrics.new_line_size;
        list.push(DisplayCommand::Text(TextFragment {
            text: line.clone(),
            rect: Rect {
                x: content.x,
                y: top,
                width: content.width,
                height: metrics.new_line_size,
            },
            baseline: top + metrics.ascent,
            color: apply_opacity(text_color, opacity),
            font_size,
            underline: false,
            strikethrough: false,
        }));
    }

    // Caret, drawn at the character offset the editing model reports.
    if focused && !disabled {
        let caret = element.caret();
        let (line, column) = crate::editing::caret_line_column(&value, caret);
        let line_text: String = value
            .split('\n')
            .nth(line)
            .unwrap_or("")
            .chars()
            .take(column)
            .collect();
        let x = content.x + measure_text(&line_text, font_size);
        let y = content.y + line as f32 * metrics.new_line_size;
        list.push(DisplayCommand::SolidColor(
            apply_opacity(CONTROL_TEXT, opacity),
            Rect {
                x,
                y: y + 1.0,
                width: 1.0,
                height: metrics.new_line_size - 2.0,
            },
        ));
    }
    list.push(DisplayCommand::PopClip);
}

/// Paint a checkbox or radio button, including its checked mark.
fn render_checkable(
    list: &mut DisplayList,
    element: &crate::dom::ElementData,
    box_rect: Rect,
    focused: bool,
    opacity: f32,
) {
    let radio = element.input_type() == "radio";
    let checked = element.is_checked();
    let disabled = element.is_disabled();
    // A radio is drawn round by giving the rectangle a full corner radius.
    let radius = if radio { box_rect.width / 2.0 } else { 3.0 };

    let background = if disabled {
        CONTROL_DISABLED
    } else if checked {
        CONTROL_FOCUS
    } else {
        CONTROL_BACKGROUND
    };
    let frame = if focused {
        CONTROL_FOCUS
    } else {
        CONTROL_BORDER
    };
    render_framed_box(
        list,
        box_rect,
        radius,
        apply_opacity(background, opacity),
        apply_opacity(frame, opacity),
        if focused { 2.0 } else { 1.0 },
    );

    if !checked {
        return;
    }
    // The mark: a dot for a radio, a filled square for a checkbox.
    let inset = box_rect.width * if radio { 0.3 } else { 0.28 };
    let mark = Rect {
        x: box_rect.x + inset,
        y: box_rect.y + inset,
        width: box_rect.width - inset * 2.0,
        height: box_rect.height - inset * 2.0,
    };
    let mark_color = if disabled {
        CONTROL_BORDER
    } else {
        CONTROL_BACKGROUND
    };
    list.push(DisplayCommand::RoundedRect(
        apply_opacity(mark_color, opacity),
        mark,
        if radio { mark.width / 2.0 } else { 1.0 },
    ));
}

/// Draw a filled box with a border, as an outer rounded rect in the border
/// colour and an inset one in the fill colour.
///
/// Painting it as a ring rather than four strips is what keeps a radio button
/// round: with `radius = width / 2` the outer rect is a circle.
fn render_framed_box(
    list: &mut DisplayList,
    rect: Rect,
    radius: f32,
    fill: Color,
    border: Color,
    border_width: f32,
) {
    list.push(DisplayCommand::RoundedRect(border, rect, radius));
    let inner = Rect {
        x: rect.x + border_width,
        y: rect.y + border_width,
        width: (rect.width - border_width * 2.0).max(0.0),
        height: (rect.height - border_width * 2.0).max(0.0),
    };
    list.push(DisplayCommand::RoundedRect(
        fill,
        inner,
        (radius - border_width).max(0.0),
    ));
}

fn render_image_with_opacity(list: &mut DisplayList, lb: &LayoutBox, opacity: f32) {
    let Some(styled) = styled_node(lb) else {
        return;
    };
    let NodeType::Element(ref element) = styled.node.node_type else {
        return;
    };
    if element.tag_name != "img" && element.tag_name != "canvas" {
        return;
    }
    let dest = lb.dimensions.content;

    // A decoded image paints its pixels; anything else falls back to alt text.
    if let Some(image) = lb.image() {
        let full = Rect {
            x: 0.0,
            y: 0.0,
            width: image.width as f32,
            height: image.height as f32,
        };
        let (dest, source) = match object_fit(styled) {
            // `contain` letterboxes: the whole image, drawn into a smaller box.
            ObjectFit::Contain => (contain_rect(image, dest), full),
            // `cover` crops: the full box, filled from part of the image.
            ObjectFit::Cover => (dest, cover_source(image, dest)),
            ObjectFit::Fill => (dest, full),
        };
        list.push(DisplayCommand::Image {
            image: image.clone(),
            dest,
            source,
            opacity,
        });
        return;
    }

    if element.tag_name == "canvas" {
        return;
    }

    let alt = element.get_attr("alt").unwrap_or("");
    list.push(DisplayCommand::SolidColor(
        apply_opacity(Color::rgb(224, 231, 239), opacity),
        dest,
    ));
    let font_size = 13.0f32.min(dest.height.max(1.0));
    list.push(DisplayCommand::Text(TextFragment {
        // Plain ASCII: the bundled font has no emoji coverage, so a pictogram
        // here would rasterize as missing-glyph boxes.
        text: crate::layout::placeholder_text(alt),
        rect: dest,
        baseline: dest.y + font_size,
        color: apply_opacity(Color::rgb(90, 105, 120), opacity),
        font_size,
        underline: false,
        strikethrough: false,
    }));
}

fn object_fit(styled: &StyledNode) -> ObjectFit {
    match styled.value("object-fit") {
        Some(Value::Keyword(k)) => match k.as_str() {
            "contain" => ObjectFit::Contain,
            "cover" => ObjectFit::Cover,
            _ => ObjectFit::Fill,
        },
        _ => ObjectFit::Fill,
    }
}

/// The centred crop of the image whose aspect ratio matches `dest`
/// (`object-fit: cover`).
fn cover_source(image: &RasterImage, dest: Rect) -> Rect {
    let full = Rect {
        x: 0.0,
        y: 0.0,
        width: image.width as f32,
        height: image.height as f32,
    };
    let Some(image_ratio) = image.aspect_ratio() else {
        return full;
    };
    if dest.width <= 0.0 || dest.height <= 0.0 {
        return full;
    }
    let dest_ratio = dest.width / dest.height;
    if image_ratio > dest_ratio {
        // Image is wider than the box: crop the sides.
        let width = full.height * dest_ratio;
        Rect {
            x: (full.width - width) / 2.0,
            y: 0.0,
            width,
            height: full.height,
        }
    } else {
        // Image is taller: crop top and bottom.
        let height = full.width / dest_ratio;
        Rect {
            x: 0.0,
            y: (full.height - height) / 2.0,
            width: full.width,
            height,
        }
    }
}

/// Shrink `dest` to the image's aspect ratio for `object-fit: contain`.
fn contain_rect(image: &RasterImage, dest: Rect) -> Rect {
    let Some(ratio) = image.aspect_ratio() else {
        return dest;
    };
    if dest.width <= 0.0 || dest.height <= 0.0 {
        return dest;
    }
    let dest_ratio = dest.width / dest.height;
    if ratio > dest_ratio {
        let height = dest.width / ratio;
        Rect {
            y: dest.y + (dest.height - height) / 2.0,
            height,
            ..dest
        }
    } else {
        let width = dest.height * ratio;
        Rect {
            x: dest.x + (dest.width - width) / 2.0,
            width,
            ..dest
        }
    }
}

fn render_text_with_opacity(list: &mut DisplayList, lb: &LayoutBox, opacity: f32) {
    for line in &lb.line_boxes {
        for fragment in &line.fragments {
            let mut f = fragment.clone();
            f.color = apply_opacity(f.color, opacity);
            list.push(DisplayCommand::Text(f));
        }
    }
}

fn get_border_radius(lb: &LayoutBox) -> f32 {
    styled_node(lb)
        .and_then(|s| s.value("border-radius"))
        .map(|v| match v {
            Value::Length(n, Unit::Px) => *n,
            Value::Length(n, Unit::Em) => n * 16.0,
            Value::Number(n) => *n,
            _ => 0.0,
        })
        .unwrap_or(0.0)
}

fn render_background_with_opacity(list: &mut DisplayList, lb: &LayoutBox, opacity: f32) {
    // Box shadow rendered first underneath background
    if let Some(Value::BoxShadow(bs)) = styled_node(lb).and_then(|s| s.value("box-shadow")) {
        list.push(DisplayCommand::BoxShadow {
            rect: lb.dimensions.border_box(),
            offset_x: bs.offset_x,
            offset_y: bs.offset_y,
            blur: bs.blur_radius,
            color: bs.color,
            opacity,
        });
    }

    // Solid background-color first (underneath gradient).
    if let Some(color) = get_color(lb, "background-color") {
        let c = apply_opacity(color, opacity);
        let rect = lb.dimensions.border_box();
        let r = get_border_radius(lb);
        if r > 0.0 {
            list.push(DisplayCommand::RoundedRect(c, rect, r));
        } else {
            list.push(DisplayCommand::SolidColor(c, rect));
        }
    }
    // Linear gradient from background-image or background shorthand (on top).
    let grad_val = styled_node(lb).and_then(|s| {
        s.value("background-image")
            .or_else(|| s.value("background"))
    });
    if let Some(Value::LinearGradient(g)) = grad_val {
        list.push(DisplayCommand::LinearGradient(
            g.clone(),
            lb.dimensions.border_box(),
            opacity,
        ));
    }
    if let Some(Value::RadialGradient(g)) = grad_val {
        list.push(DisplayCommand::RadialGradient(
            g.clone(),
            lb.dimensions.border_box(),
            opacity,
        ));
    }
    if let Some(Value::ConicGradient(g)) = grad_val {
        list.push(DisplayCommand::ConicGradient(
            g.clone(),
            lb.dimensions.border_box(),
            opacity,
        ));
    }
}

fn render_borders_with_opacity(list: &mut DisplayList, lb: &LayoutBox, opacity: f32) {
    let color = match get_color(lb, "border-color") {
        Some(c) => apply_opacity(c, opacity),
        None => return,
    };
    let d = &lb.dimensions;
    let bb = d.border_box();

    if d.border.top > 0.0 {
        list.push(DisplayCommand::SolidColor(
            color,
            Rect {
                x: bb.x,
                y: bb.y,
                width: bb.width,
                height: d.border.top,
            },
        ));
    }
    if d.border.bottom > 0.0 {
        list.push(DisplayCommand::SolidColor(
            color,
            Rect {
                x: bb.x,
                y: bb.y + bb.height - d.border.bottom,
                width: bb.width,
                height: d.border.bottom,
            },
        ));
    }
    if d.border.left > 0.0 {
        list.push(DisplayCommand::SolidColor(
            color,
            Rect {
                x: bb.x,
                y: bb.y,
                width: d.border.left,
                height: bb.height,
            },
        ));
    }
    if d.border.right > 0.0 {
        list.push(DisplayCommand::SolidColor(
            color,
            Rect {
                x: bb.x + bb.width - d.border.right,
                y: bb.y,
                width: d.border.right,
                height: bb.height,
            },
        ));
    }
}

fn get_color(lb: &LayoutBox, name: &str) -> Option<Color> {
    match styled_node(lb)?.value(name) {
        Some(Value::Color(c)) if c.a > 0 => Some(*c),
        _ => None,
    }
}

fn styled_node<'layout, 'style>(
    lb: &'layout LayoutBox<'style>,
) -> Option<&'layout StyledNode<'style>> {
    match &lb.box_type {
        BoxType::Block(s)
        | BoxType::Flex(s)
        | BoxType::Grid(s)
        | BoxType::Table(s)
        | BoxType::TableRow(s)
        | BoxType::TableCell(s)
        | BoxType::Inline(s)
        | BoxType::InlineBlock(s) => Some(*s),
        BoxType::AnonymousBlock => None,
    }
}

fn is_positioned(lb: &LayoutBox) -> bool {
    styled_node(lb).is_some_and(|style| style.position() != Position::Static)
}

fn establishes_stacking_context(lb: &LayoutBox) -> bool {
    styled_node(lb).is_some_and(StyledNode::establishes_stacking_context)
}

// ── Canvas ────────────────────────────────────────────────────────────────────

pub struct Canvas {
    pub pixels: Vec<u8>, // flat RGB: pixel (x,y) → pixels[(y*width+x)*3..]
    pub width: usize,
    pub height: usize,
    /// Each entry is the effective (intersected) clip rect as (x0,y0,x1,y1).
    clip_stack: Vec<(i32, i32, i32, i32)>,
}

impl Canvas {
    pub fn new(width: usize, height: usize, background: Color) -> Self {
        let mut pixels = vec![0u8; width * height * 3];
        for i in (0..pixels.len()).step_by(3) {
            pixels[i] = background.r;
            pixels[i + 1] = background.g;
            pixels[i + 2] = background.b;
        }
        Self {
            pixels,
            width,
            height,
            clip_stack: Vec::new(),
        }
    }

    pub fn paint(&mut self, item: &DisplayCommand) {
        match item {
            DisplayCommand::SolidColor(color, rect) => {
                let x0 = clamp(rect.x as i32, 0, self.width as i32);
                let y0 = clamp(rect.y as i32, 0, self.height as i32);
                let x1 = clamp((rect.x + rect.width) as i32, 0, self.width as i32);
                let y1 = clamp((rect.y + rect.height) as i32, 0, self.height as i32);
                for y in y0..y1 {
                    for x in x0..x1 {
                        self.blend_pixel(x, y, *color, color.a);
                    }
                }
            }
            DisplayCommand::RoundedRect(color, rect, radius) => {
                self.paint_rounded_rect(*color, *rect, *radius);
            }
            DisplayCommand::Text(fragment) => {
                self.paint_text(fragment);
            }
            DisplayCommand::PushClip(rect) => {
                let nx0 = clamp(rect.x as i32, 0, self.width as i32);
                let ny0 = clamp(rect.y as i32, 0, self.height as i32);
                let nx1 = clamp((rect.x + rect.width) as i32, 0, self.width as i32);
                let ny1 = clamp((rect.y + rect.height) as i32, 0, self.height as i32);
                let clip = if let Some(&prev) = self.clip_stack.last() {
                    (
                        nx0.max(prev.0),
                        ny0.max(prev.1),
                        nx1.min(prev.2),
                        ny1.min(prev.3),
                    )
                } else {
                    (nx0, ny0, nx1, ny1)
                };
                self.clip_stack.push(clip);
            }
            DisplayCommand::PopClip => {
                self.clip_stack.pop();
            }
            DisplayCommand::Image {
                image,
                dest,
                source,
                opacity,
            } => {
                self.paint_image(image, *dest, *source, *opacity);
            }
            DisplayCommand::LinearGradient(grad, rect, opacity) => {
                self.paint_linear_gradient(grad, *rect, *opacity);
            }
            DisplayCommand::RadialGradient(grad, rect, opacity) => {
                self.paint_radial_gradient(grad, *rect, *opacity);
            }
            DisplayCommand::ConicGradient(grad, rect, opacity) => {
                self.paint_conic_gradient(grad, *rect, *opacity);
            }
            DisplayCommand::BoxShadow {
                rect,
                offset_x,
                offset_y,
                blur,
                color,
                opacity,
            } => {
                self.paint_box_shadow(*rect, *offset_x, *offset_y, *blur, *color, *opacity);
            }
        }
    }

    fn paint_box_shadow(
        &mut self,
        rect: Rect,
        offset_x: f32,
        offset_y: f32,
        blur: f32,
        color: Color,
        opacity: f32,
    ) {
        let shadow_rect = Rect {
            x: rect.x + offset_x,
            y: rect.y + offset_y,
            width: rect.width,
            height: rect.height,
        };
        let margin = blur.max(0.0) * 1.5;
        let x0 = clamp((shadow_rect.x - margin) as i32, 0, self.width as i32);
        let y0 = clamp((shadow_rect.y - margin) as i32, 0, self.height as i32);
        let x1 = clamp(
            (shadow_rect.x + shadow_rect.width + margin) as i32,
            0,
            self.width as i32,
        );
        let y1 = clamp(
            (shadow_rect.y + shadow_rect.height + margin) as i32,
            0,
            self.height as i32,
        );

        for y in y0..y1 {
            for x in x0..x1 {
                let fx = x as f32 + 0.5;
                let fy = y as f32 + 0.5;

                let dx = (shadow_rect.x - fx)
                    .max(0.0)
                    .max(fx - (shadow_rect.x + shadow_rect.width));
                let dy = (shadow_rect.y - fy)
                    .max(0.0)
                    .max(fy - (shadow_rect.y + shadow_rect.height));
                let dist = (dx * dx + dy * dy).sqrt();

                let factor = if blur > 0.0 {
                    (1.0 - dist / blur).clamp(0.0, 1.0)
                } else if dist == 0.0 {
                    1.0
                } else {
                    0.0
                };

                if factor > 0.0 {
                    let alpha = (color.a as f32 * opacity * factor) as u8;
                    self.blend_pixel(x, y, color, alpha);
                }
            }
        }
    }

    fn paint_rounded_rect(&mut self, color: Color, rect: Rect, radius: f32) {
        let r = radius.min(rect.width / 2.0).min(rect.height / 2.0).max(0.0);
        let x0 = clamp(rect.x as i32, 0, self.width as i32);
        let y0 = clamp(rect.y as i32, 0, self.height as i32);
        let x1 = clamp((rect.x + rect.width) as i32, 0, self.width as i32);
        let y1 = clamp((rect.y + rect.height) as i32, 0, self.height as i32);
        let cl = rect.x + r; // corner-left threshold
        let cr = rect.x + rect.width - r;
        let ct = rect.y + r;
        let cb = rect.y + rect.height - r;

        for y in y0..y1 {
            for x in x0..x1 {
                let fx = x as f32 + 0.5;
                let fy = y as f32 + 0.5;
                let in_corner = (fx < cl || fx > cr) && (fy < ct || fy > cb);
                if in_corner {
                    let cx = if fx < cl { cl } else { cr };
                    let cy = if fy < ct { ct } else { cb };
                    if (fx - cx) * (fx - cx) + (fy - cy) * (fy - cy) > r * r {
                        continue;
                    }
                }
                self.blend_pixel(x, y, color, color.a);
            }
        }
    }

    /// Scale `source` (image pixels) onto `dest` (canvas pixels) with bilinear
    /// sampling and alpha blending.
    fn paint_image(&mut self, image: &RasterImage, dest: Rect, source: Rect, opacity: f32) {
        if image.width == 0 || image.height == 0 || dest.width <= 0.0 || dest.height <= 0.0 {
            return;
        }
        let x0 = clamp(dest.x.floor() as i32, 0, self.width as i32);
        let y0 = clamp(dest.y.floor() as i32, 0, self.height as i32);
        let x1 = clamp((dest.x + dest.width).ceil() as i32, 0, self.width as i32);
        let y1 = clamp((dest.y + dest.height).ceil() as i32, 0, self.height as i32);

        for y in y0..y1 {
            // Sample at the centre of each destination pixel.
            let v = (y as f32 + 0.5 - dest.y) / dest.height;
            let src_y = source.y + v * source.height - 0.5;
            for x in x0..x1 {
                let u = (x as f32 + 0.5 - dest.x) / dest.width;
                let src_x = source.x + u * source.width - 0.5;
                let [r, g, b, a] = sample_bilinear(image, src_x, src_y);
                let alpha = (a as f32 * opacity) as u8;
                if alpha == 0 {
                    continue;
                }
                self.blend_pixel(x, y, Color { r, g, b, a: alpha }, alpha);
            }
        }
    }

    fn paint_linear_gradient(&mut self, grad: &LinearGradient, rect: Rect, opacity: f32) {
        if grad.stops.is_empty() {
            return;
        }

        let angle_rad = grad.angle_deg * std::f32::consts::PI / 180.0;
        let dx = angle_rad.sin();
        let dy = -angle_rad.cos(); // y increases downward in screen coords

        let cx = rect.x + rect.width / 2.0;
        let cy = rect.y + rect.height / 2.0;
        // Half-length of the gradient line that covers the full rect.
        let half_len = (rect.width / 2.0 * dx.abs() + rect.height / 2.0 * dy.abs()).max(1.0);

        let stops = resolve_gradient_stops(&grad.stops);

        let x0 = clamp(rect.x as i32, 0, self.width as i32);
        let y0 = clamp(rect.y as i32, 0, self.height as i32);
        let x1 = clamp((rect.x + rect.width) as i32, 0, self.width as i32);
        let y1 = clamp((rect.y + rect.height) as i32, 0, self.height as i32);

        for y in y0..y1 {
            for x in x0..x1 {
                // Clip check (mirrors blend_pixel, but avoids function call overhead).
                if let Some(&(cx0, cy0, cx1, cy1)) = self.clip_stack.last() {
                    if x < cx0 || x >= cx1 || y < cy0 || y >= cy1 {
                        continue;
                    }
                }
                let rpx = x as f32 + 0.5 - cx;
                let rpy = y as f32 + 0.5 - cy;
                let proj = rpx * dx + rpy * dy;
                let t = ((proj + half_len) / (2.0 * half_len)).clamp(0.0, 1.0);
                let color = interp_gradient_stops(&stops, t);
                let a = (color.a as f32 * opacity) as u8;
                if a == 0 {
                    continue;
                }
                self.blend_pixel(x, y, Color { a, ..color }, a);
            }
        }
    }

    fn paint_radial_gradient(&mut self, grad: &RadialGradient, rect: Rect, opacity: f32) {
        if grad.stops.is_empty() {
            return;
        }

        let cx = rect.x + rect.width / 2.0;
        let cy = rect.y + rect.height / 2.0;
        let max_r = ((rect.width / 2.0).powi(2) + (rect.height / 2.0).powi(2)).sqrt().max(1.0);

        let stops = resolve_gradient_stops(&grad.stops);

        let x0 = clamp(rect.x as i32, 0, self.width as i32);
        let y0 = clamp(rect.y as i32, 0, self.height as i32);
        let x1 = clamp((rect.x + rect.width) as i32, 0, self.width as i32);
        let y1 = clamp((rect.y + rect.height) as i32, 0, self.height as i32);

        for y in y0..y1 {
            for x in x0..x1 {
                if let Some(&(cx0, cy0, cx1, cy1)) = self.clip_stack.last() {
                    if x < cx0 || x >= cx1 || y < cy0 || y >= cy1 {
                        continue;
                    }
                }
                let rpx = x as f32 + 0.5 - cx;
                let rpy = y as f32 + 0.5 - cy;
                let dist = (rpx * rpx + rpy * rpy).sqrt();
                let t = (dist / max_r).clamp(0.0, 1.0);
                let color = interp_gradient_stops(&stops, t);
                let a = (color.a as f32 * opacity) as u8;
                if a == 0 {
                    continue;
                }
                self.blend_pixel(x, y, Color { a, ..color }, a);
            }
        }
    }

    fn paint_conic_gradient(&mut self, grad: &ConicGradient, rect: Rect, opacity: f32) {
        if grad.stops.is_empty() {
            return;
        }

        let cx = rect.x + rect.width / 2.0;
        let cy = rect.y + rect.height / 2.0;
        let stops = resolve_gradient_stops(&grad.stops);

        let x0 = clamp(rect.x as i32, 0, self.width as i32);
        let y0 = clamp(rect.y as i32, 0, self.height as i32);
        let x1 = clamp((rect.x + rect.width) as i32, 0, self.width as i32);
        let y1 = clamp((rect.y + rect.height) as i32, 0, self.height as i32);

        for y in y0..y1 {
            for x in x0..x1 {
                if let Some(&(cx0, cy0, cx1, cy1)) = self.clip_stack.last() {
                    if x < cx0 || x >= cx1 || y < cy0 || y >= cy1 {
                        continue;
                    }
                }
                let rpx = x as f32 + 0.5 - cx;
                let rpy = y as f32 + 0.5 - cy;
                let mut angle_deg = rpy.atan2(rpx) * 180.0 / std::f32::consts::PI + 90.0;
                if angle_deg < 0.0 {
                    angle_deg += 360.0;
                }
                angle_deg = (angle_deg - grad.from_angle_deg).rem_euclid(360.0);
                let t = (angle_deg / 360.0).clamp(0.0, 1.0);
                let color = interp_gradient_stops(&stops, t);
                let a = (color.a as f32 * opacity) as u8;
                if a == 0 {
                    continue;
                }
                self.blend_pixel(x, y, Color { a, ..color }, a);
            }
        }
    }

    fn paint_text(&mut self, fragment: &TextFragment) {
        let mut pen_x = fragment.rect.x;
        for character in fragment.text.chars() {
            let Some((metrics, bitmap)) = rasterize(character, fragment.font_size) else {
                pen_x += fragment.font_size * 0.6; // fallback advance
                continue;
            };
            let glyph_x = pen_x.round() as i32 + metrics.xmin;
            let glyph_y = fragment.baseline.round() as i32 - metrics.height as i32 - metrics.ymin;
            for row in 0..metrics.height {
                for col in 0..metrics.width {
                    let coverage = bitmap[row * metrics.width + col];
                    if coverage > 0 {
                        self.blend_pixel(
                            glyph_x + col as i32,
                            glyph_y + row as i32,
                            fragment.color,
                            coverage,
                        );
                    }
                }
            }
            pen_x += metrics.advance_width;
        }
        // Text decorations — drawn after all glyphs.
        if fragment.underline || fragment.strikethrough {
            let x0 = fragment.rect.x as i32;
            let x1 = (fragment.rect.x + fragment.rect.width) as i32;
            let thickness = (fragment.font_size / 14.0).max(1.0) as i32;
            if fragment.underline {
                let y = fragment.baseline.round() as i32 + 1;
                for dy in 0..thickness {
                    for x in x0..x1 {
                        self.blend_pixel(x, y + dy, fragment.color, 255);
                    }
                }
            }
            if fragment.strikethrough {
                let y = (fragment.baseline - fragment.font_size * 0.35).round() as i32;
                for dy in 0..thickness {
                    for x in x0..x1 {
                        self.blend_pixel(x, y + dy, fragment.color, 255);
                    }
                }
            }
        }
    }

    fn blend_pixel(&mut self, x: i32, y: i32, color: Color, coverage: u8) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        // Respect the current clip rectangle.
        if let Some(&(cx0, cy0, cx1, cy1)) = self.clip_stack.last() {
            if x < cx0 || x >= cx1 || y < cy0 || y >= cy1 {
                return;
            }
        }
        let idx = (y as usize * self.width + x as usize) * 3;
        let alpha = color.a as f32 / 255.0 * coverage as f32 / 255.0;
        let ia = 1.0 - alpha;
        self.pixels[idx] = (color.r as f32 * alpha + self.pixels[idx] as f32 * ia) as u8;
        self.pixels[idx + 1] = (color.g as f32 * alpha + self.pixels[idx + 1] as f32 * ia) as u8;
        self.pixels[idx + 2] = (color.b as f32 * alpha + self.pixels[idx + 2] as f32 * ia) as u8;
    }

    pub fn to_u32_buffer(&self) -> Vec<u32> {
        self.pixels
            .chunks_exact(3)
            .map(|rgb| u32::from(rgb[0]) << 16 | u32::from(rgb[1]) << 8 | u32::from(rgb[2]))
            .collect()
    }

    /// Encode the canvas as a PPM P6 binary image (bytes, not a UTF-8 String).
    pub fn to_ppm(&self) -> Vec<u8> {
        let header = format!("P6\n{} {}\n255\n", self.width, self.height);
        let mut out = Vec::with_capacity(header.len() + self.pixels.len());
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(&self.pixels);
        out
    }
}

fn clamp(v: i32, lo: i32, hi: i32) -> i32 {
    v.max(lo).min(hi)
}

/// Bilinear sample of an image at a (possibly fractional) pixel coordinate.
///
/// Alpha is weighted into the colour channels so that transparent pixels do
/// not bleed their colour into the edges of an upscaled sprite.
fn sample_bilinear(image: &RasterImage, x: f32, y: f32) -> [u8; 4] {
    let x = x.clamp(0.0, (image.width.saturating_sub(1)) as f32);
    let y = y.clamp(0.0, (image.height.saturating_sub(1)) as f32);
    let (x0, y0) = (x.floor() as u32, y.floor() as u32);
    let (x1, y1) = (x0 + 1, y0 + 1);
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);

    let corners = [
        (image.pixel(x0, y0), (1.0 - fx) * (1.0 - fy)),
        (image.pixel(x1, y0), fx * (1.0 - fy)),
        (image.pixel(x0, y1), (1.0 - fx) * fy),
        (image.pixel(x1, y1), fx * fy),
    ];

    let mut alpha = 0.0f32;
    let mut rgb = [0.0f32; 3];
    for (pixel, weight) in corners {
        let a = pixel[3] as f32 / 255.0;
        alpha += a * weight;
        for channel in 0..3 {
            rgb[channel] += pixel[channel] as f32 * a * weight;
        }
    }

    if alpha <= 0.0 {
        return [0, 0, 0, 0];
    }
    [
        (rgb[0] / alpha).round().clamp(0.0, 255.0) as u8,
        (rgb[1] / alpha).round().clamp(0.0, 255.0) as u8,
        (rgb[2] / alpha).round().clamp(0.0, 255.0) as u8,
        (alpha * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

fn resolve_gradient_stops(stops: &[ColorStop]) -> Vec<(f32, Color)> {
    let n = stops.len();
    stops
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let pos = s.position.unwrap_or(if n <= 1 {
                0.0
            } else {
                i as f32 / (n - 1) as f32
            });
            (pos, s.color)
        })
        .collect()
}

fn interp_gradient_stops(stops: &[(f32, Color)], t: f32) -> Color {
    if stops.is_empty() {
        return Color::rgb(0, 0, 0);
    }
    if stops.len() == 1 {
        return stops[0].1;
    }
    if t <= stops[0].0 {
        return stops[0].1;
    }
    if t >= stops.last().unwrap().0 {
        return stops.last().unwrap().1;
    }
    for i in 0..stops.len() - 1 {
        let (t0, c0) = stops[i];
        let (t1, c1) = stops[i + 1];
        if t >= t0 && t <= t1 {
            let f = if (t1 - t0).abs() < 1e-6 {
                0.0
            } else {
                (t - t0) / (t1 - t0)
            };
            return Color {
                r: lerp_u8(c0.r, c1.r, f),
                g: lerp_u8(c0.g, c1.g, f),
                b: lerp_u8(c0.b, c1.b, f),
                a: lerp_u8(c0.a, c1.a, f),
            };
        }
    }
    stops.last().unwrap().1
}

#[inline]
fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + t * (b as f32 - a as f32)).round() as u8
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Paint `layout_root` onto a canvas of `width × height` with a white background.
pub fn paint(layout_root: &LayoutBox, width: usize, height: usize) -> Canvas {
    paint_with_scroll(layout_root, width, height, 0.0, 0.0)
}

/// Paint `layout_root` onto a canvas of `width × height` with scroll offset.
pub fn paint_with_scroll(
    layout_root: &LayoutBox,
    width: usize,
    height: usize,
    scroll_x: f32,
    scroll_y: f32,
) -> Canvas {
    let bg = Color::rgb(255, 255, 255);
    let mut canvas = Canvas::new(width, height, bg);
    let list = build_display_list(layout_root);
    for cmd in &list {
        let mut scrolled_cmd = cmd.clone();
        scrolled_cmd.offset(-scroll_x, -scroll_y);
        canvas.paint(&scrolled_cmd);
    }
    canvas
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::parser::parse_css;
    use crate::html::parse_html;
    use crate::layout::layout_tree;
    use crate::style::style_tree;

    fn render_to_canvas(html: &str, css: &str, w: usize, h: usize) -> Canvas {
        let dom = Box::leak(Box::new(parse_html(html)));
        let ss = Box::leak(Box::new(parse_css(css)));
        let styled = Box::leak(Box::new(style_tree(dom, ss)));
        let layout = Box::leak(Box::new(layout_tree(styled, w as f32)));
        paint(layout, w, h)
    }

    fn display_list_for(html: &str, css: &str, w: usize) -> DisplayList {
        let dom = parse_html(html);
        let ss = parse_css(css);
        let styled = style_tree(&dom, &ss);
        let layout = layout_tree(&styled, w as f32);
        build_display_list(&layout)
    }

    fn solid_colors(list: &DisplayList) -> Vec<Color> {
        list.iter()
            .filter_map(|command| match command {
                DisplayCommand::SolidColor(color, _) => Some(*color),
                DisplayCommand::RoundedRect(color, _, _) => Some(*color),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn background_color_painted() {
        let canvas = render_to_canvas(
            "<div></div>",
            "div { display: block; background-color: #ff0000; height: 100px; }",
            100,
            200,
        );
        // Pixel (50, 50) should be red.
        let idx = (50 * 100 + 50) * 3;
        assert_eq!(canvas.pixels[idx], 255); // R
        assert_eq!(canvas.pixels[idx + 1], 0); // G
        assert_eq!(canvas.pixels[idx + 2], 0); // B
    }

    #[test]
    fn ppm_header_correct() {
        let c = Canvas::new(4, 2, Color::rgb(0, 0, 0));
        let ppm = c.to_ppm();
        let header_end = ppm.iter().position(|&b| b == b'\n').unwrap() + 1;
        let rest = &ppm[..header_end + 10];
        assert!(std::str::from_utf8(rest).unwrap().starts_with("P6\n"));
    }

    #[test]
    fn glyphs_are_rasterized() {
        if crate::text::rasterize('H', 24.0).is_none() {
            return;
        }
        let canvas = render_to_canvas(
            "<p>Hello</p>",
            "p { display: block; color: red; font-size: 24px; }",
            200,
            60,
        );
        assert!(canvas
            .pixels
            .chunks_exact(3)
            .any(|pixel| pixel != [255, 255, 255]));
    }

    #[test]
    fn converts_rgb_pixels_for_minifb() {
        let canvas = Canvas::new(1, 1, Color::rgb(0x12, 0x34, 0x56));
        assert_eq!(canvas.to_u32_buffer(), vec![0x123456]);
    }

    #[test]
    fn stacking_layers_paint_negative_flow_and_positive_in_order() {
        let list = display_list_for(
            r#"
                <div class="positive"></div>
                <div class="flow"></div>
                <div class="negative"></div>
            "#,
            r#"
                div { display: block; height: 10px; }
                .positive { position: relative; z-index: 2; background-color: red; }
                .flow { background-color: green; }
                .negative { position: relative; z-index: -2; background-color: blue; }
            "#,
            100,
        );
        assert_eq!(
            solid_colors(&list),
            vec![
                Color::rgb(0, 0, 255),
                Color::rgb(0, 128, 0),
                Color::rgb(255, 0, 0),
            ]
        );
    }

    #[test]
    fn equal_z_index_keeps_dom_order() {
        let list = display_list_for(
            r#"<div class="first"></div><div class="second"></div>"#,
            r#"
                div { display: block; position: relative; z-index: 1; height: 10px; }
                .first { background-color: red; }
                .second { background-color: blue; }
            "#,
            100,
        );
        assert_eq!(
            solid_colors(&list),
            vec![Color::rgb(255, 0, 0), Color::rgb(0, 0, 255)]
        );
    }

    #[test]
    fn positioned_auto_layer_paints_after_normal_flow() {
        let list = display_list_for(
            r#"<div class="auto"></div><div class="flow"></div>"#,
            r#"
                div { display: block; height: 10px; }
                .auto { position: relative; background-color: blue; }
                .flow { background-color: green; }
            "#,
            100,
        );
        assert_eq!(
            solid_colors(&list),
            vec![Color::rgb(0, 128, 0), Color::rgb(0, 0, 255)]
        );
    }

    #[test]
    fn nested_stacking_context_is_atomic_against_siblings() {
        let list = display_list_for(
            r#"
                <div class="outer"><div class="inner"></div></div>
                <div class="sibling"></div>
            "#,
            r#"
                div { display: block; position: relative; height: 10px; }
                .outer { z-index: 1; background-color: red; }
                .inner { z-index: 999; background-color: green; }
                .sibling { z-index: 2; background-color: blue; }
            "#,
            100,
        );
        assert_eq!(
            solid_colors(&list),
            vec![
                Color::rgb(255, 0, 0),
                Color::rgb(0, 128, 0),
                Color::rgb(0, 0, 255),
            ]
        );
    }

    #[test]
    fn border_radius_emits_rounded_rect_command() {
        let list = display_list_for(
            "<div></div>",
            "div { display: block; height: 50px; background-color: red; border-radius: 8px; }",
            100,
        );
        let has_rounded = list
            .iter()
            .any(|cmd| matches!(cmd, DisplayCommand::RoundedRect(_, _, r) if *r > 0.0));
        assert!(
            has_rounded,
            "expected a RoundedRect command for border-radius"
        );
    }

    #[test]
    fn overflow_hidden_emits_push_pop_clip() {
        let list = display_list_for(
            "<div><p>text</p></div>",
            "div { display: block; height: 50px; overflow: hidden; }",
            100,
        );
        let has_push = list
            .iter()
            .any(|cmd| matches!(cmd, DisplayCommand::PushClip(_)));
        let has_pop = list
            .iter()
            .any(|cmd| matches!(cmd, DisplayCommand::PopClip));
        assert!(has_push, "expected PushClip for overflow:hidden");
        assert!(has_pop, "expected PopClip for overflow:hidden");
    }

    #[test]
    fn list_marker_emits_text_command() {
        let list = display_list_for(
            "<ul><li>item</li></ul>",
            "ul { display: block; list-style-type: disc; } li { display: block; }",
            200,
        );
        // Expect at least one text fragment containing the bullet character.
        let has_bullet = list
            .iter()
            .any(|cmd| matches!(cmd, DisplayCommand::Text(f) if f.text.contains('•')));
        assert!(has_bullet, "expected bullet marker text fragment for <li>");
    }

    #[test]
    fn opacity_reduces_alpha_in_display_list() {
        let list = display_list_for(
            r#"<div></div>"#,
            r#"div { display: block; height: 50px; background-color: red; opacity: 0.5; }"#,
            100,
        );
        let colors: Vec<Color> = list
            .iter()
            .filter_map(|cmd| {
                if let DisplayCommand::SolidColor(c, _) = cmd {
                    Some(*c)
                } else {
                    None
                }
            })
            .collect();
        assert!(!colors.is_empty(), "expected at least one solid color");
        // alpha should be approximately 255 * 0.5 = 127
        assert!(
            colors[0].a < 200,
            "opacity 0.5 should reduce alpha (got {})",
            colors[0].a
        );
    }

    #[test]
    fn linear_gradient_paints_left_right_colors() {
        // to right: left edge = red, right edge = blue, middle = mix
        let canvas = render_to_canvas(
            "<div></div>",
            "div { display: block; height: 100px; background-image: linear-gradient(to right, red, blue); }",
            200, 100,
        );
        let left_idx = (50 * 200 + 5) * 3;
        let right_idx = (50 * 200 + 194) * 3;
        // Left pixel: red channel dominant
        assert!(
            canvas.pixels[left_idx] > 200,
            "left pixel R should be high, got {}",
            canvas.pixels[left_idx]
        );
        assert!(
            canvas.pixels[left_idx + 2] < 50,
            "left pixel B should be low, got {}",
            canvas.pixels[left_idx + 2]
        );
        // Right pixel: blue channel dominant
        assert!(
            canvas.pixels[right_idx + 2] > 200,
            "right pixel B should be high, got {}",
            canvas.pixels[right_idx + 2]
        );
        assert!(
            canvas.pixels[right_idx] < 50,
            "right pixel R should be low, got {}",
            canvas.pixels[right_idx]
        );
    }

    #[test]
    fn linear_gradient_emits_display_command() {
        let list = display_list_for(
            "<div></div>",
            "div { display: block; height: 50px; background-image: linear-gradient(to bottom, red, blue); }",
            100,
        );
        let has_grad = list
            .iter()
            .any(|cmd| matches!(cmd, DisplayCommand::LinearGradient(_, _, _)));
        assert!(has_grad, "expected LinearGradient display command");
    }

    #[test]
    fn z_index_controls_overlapping_pixels() {
        let canvas = render_to_canvas(
            r#"<div class="high"></div><div class="low"></div>"#,
            r#"
                div { display: block; height: 100px; }
                .high { position: relative; z-index: 10; background-color: red; }
                .low {
                    position: relative;
                    z-index: -1;
                    margin-top: -100px;
                    background-color: blue;
                }
            "#,
            100,
            100,
        );
        let idx = (50 * 100 + 50) * 3;
        assert_eq!(&canvas.pixels[idx..idx + 3], &[255, 0, 0]);
    }

    #[test]
    fn box_shadow_emits_display_command() {
        let list = display_list_for(
            r#"<div class="card"></div>"#,
            r#".card { display: block; width: 100px; height: 100px; box-shadow: 2px 4px 8px #000000; }"#,
            200,
        );
        let has_shadow = list
            .iter()
            .any(|cmd| matches!(cmd, DisplayCommand::BoxShadow { .. }));
        assert!(has_shadow, "box-shadow should emit BoxShadow command");
    }
}
