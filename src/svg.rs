// ============================================================
//  svg.rs  —  SVG Vector Graphics Parser and Rasterizer
// ============================================================
//
//  Parses SVG DOM subtrees (<svg>, <rect>, <circle>, <line>, <polygon>, <path>)
//  and renders them into rasterized vector pixel buffers using CanvasContext2D.

use crate::canvas::CanvasContext2D;
use crate::css::parser::{named_color, parse_color, Color};
use crate::dom::Node;
use crate::image::RasterImage;
use std::rc::Rc;

#[derive(Debug, Clone, PartialEq)]
pub struct SvgViewBox {
    pub min_x: f32,
    pub min_y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SvgPathSegment {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    CubicCurveTo {
        cp1x: f32,
        cp1y: f32,
        cp2x: f32,
        cp2y: f32,
        x: f32,
        y: f32,
    },
    QuadraticCurveTo {
        cpx: f32,
        cpy: f32,
        x: f32,
        y: f32,
    },
    ClosePath,
}

pub fn parse_svg_color(color_str: &str) -> Option<Color> {
    let trimmed = color_str.trim();
    if trimmed.eq_ignore_ascii_case("none") {
        return Some(Color::transparent());
    }
    if let Some(c) = parse_color(trimmed) {
        return Some(c);
    }
    named_color(trimmed)
}

pub fn parse_view_box(val: &str) -> Option<SvgViewBox> {
    let nums: Vec<f32> = val
        .split(|c: char| c.is_ascii_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<f32>().ok())
        .collect();
    if nums.len() == 4 && nums[2] > 0.0 && nums[3] > 0.0 {
        Some(SvgViewBox {
            min_x: nums[0],
            min_y: nums[1],
            width: nums[2],
            height: nums[3],
        })
    } else {
        None
    }
}

pub fn parse_points(val: &str) -> Vec<(f32, f32)> {
    let nums: Vec<f32> = val
        .split(|c: char| c.is_ascii_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<f32>().ok())
        .collect();
    nums.chunks_exact(2)
        .map(|pair| (pair[0], pair[1]))
        .collect()
}

pub fn parse_svg_path_data(d: &str) -> Vec<SvgPathSegment> {
    let mut segments = Vec::new();
    let chars: Vec<char> = d.chars().collect();
    let mut i = 0;

    let mut cur_x = 0.0f32;
    let mut cur_y = 0.0f32;
    let mut start_x = 0.0f32;
    let mut start_y = 0.0f32;

    let next_num = |idx: &mut usize| -> Option<f32> {
        while *idx < chars.len() && (chars[*idx].is_ascii_whitespace() || chars[*idx] == ',') {
            *idx += 1;
        }
        if *idx >= chars.len() {
            return None;
        }
        let start = *idx;
        if chars[*idx] == '+' || chars[*idx] == '-' {
            *idx += 1;
        }
        let mut has_digits = false;
        while *idx < chars.len() && (chars[*idx].is_ascii_digit() || chars[*idx] == '.') {
            has_digits = true;
            *idx += 1;
        }
        if has_digits {
            let num_s: String = chars[start..*idx].iter().collect();
            num_s.parse::<f32>().ok()
        } else {
            None
        }
    };

    while i < chars.len() {
        while i < chars.len() && (chars[i].is_ascii_whitespace() || chars[i] == ',') {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }

        let cmd = chars[i];
        if !cmd.is_ascii_alphabetic() {
            i += 1;
            continue;
        }
        i += 1;

        match cmd {
            'M' => {
                if let (Some(x), Some(y)) = (next_num(&mut i), next_num(&mut i)) {
                    cur_x = x;
                    cur_y = y;
                    start_x = x;
                    start_y = y;
                    segments.push(SvgPathSegment::MoveTo(x, y));
                    // Subsequent coordinate pairs are treated as implicit LineTo commands
                    while let (Some(lx), Some(ly)) = (next_num(&mut i), next_num(&mut i)) {
                        cur_x = lx;
                        cur_y = ly;
                        segments.push(SvgPathSegment::LineTo(lx, ly));
                    }
                }
            }
            'm' => {
                if let (Some(dx), Some(dy)) = (next_num(&mut i), next_num(&mut i)) {
                    cur_x += dx;
                    cur_y += dy;
                    start_x = cur_x;
                    start_y = cur_y;
                    segments.push(SvgPathSegment::MoveTo(cur_x, cur_y));
                    while let (Some(dlx), Some(dly)) = (next_num(&mut i), next_num(&mut i)) {
                        cur_x += dlx;
                        cur_y += dly;
                        segments.push(SvgPathSegment::LineTo(cur_x, cur_y));
                    }
                }
            }
            'L' => {
                while let (Some(x), Some(y)) = (next_num(&mut i), next_num(&mut i)) {
                    cur_x = x;
                    cur_y = y;
                    segments.push(SvgPathSegment::LineTo(x, y));
                }
            }
            'l' => {
                while let (Some(dx), Some(dy)) = (next_num(&mut i), next_num(&mut i)) {
                    cur_x += dx;
                    cur_y += dy;
                    segments.push(SvgPathSegment::LineTo(cur_x, cur_y));
                }
            }
            'H' => {
                while let Some(x) = next_num(&mut i) {
                    cur_x = x;
                    segments.push(SvgPathSegment::LineTo(cur_x, cur_y));
                }
            }
            'h' => {
                while let Some(dx) = next_num(&mut i) {
                    cur_x += dx;
                    segments.push(SvgPathSegment::LineTo(cur_x, cur_y));
                }
            }
            'V' => {
                while let Some(y) = next_num(&mut i) {
                    cur_y = y;
                    segments.push(SvgPathSegment::LineTo(cur_x, cur_y));
                }
            }
            'v' => {
                while let Some(dy) = next_num(&mut i) {
                    cur_y += dy;
                    segments.push(SvgPathSegment::LineTo(cur_x, cur_y));
                }
            }
            'C' => {
                while let (Some(cp1x), Some(cp1y), Some(cp2x), Some(cp2y), Some(x), Some(y)) = (
                    next_num(&mut i),
                    next_num(&mut i),
                    next_num(&mut i),
                    next_num(&mut i),
                    next_num(&mut i),
                    next_num(&mut i),
                ) {
                    cur_x = x;
                    cur_y = y;
                    segments.push(SvgPathSegment::CubicCurveTo {
                        cp1x,
                        cp1y,
                        cp2x,
                        cp2y,
                        x,
                        y,
                    });
                }
            }
            'c' => {
                while let (Some(dcp1x), Some(dcp1y), Some(dcp2x), Some(dcp2y), Some(dx), Some(dy)) = (
                    next_num(&mut i),
                    next_num(&mut i),
                    next_num(&mut i),
                    next_num(&mut i),
                    next_num(&mut i),
                    next_num(&mut i),
                ) {
                    let cp1x = cur_x + dcp1x;
                    let cp1y = cur_y + dcp1y;
                    let cp2x = cur_x + dcp2x;
                    let cp2y = cur_y + dcp2y;
                    let x = cur_x + dx;
                    let y = cur_y + dy;
                    cur_x = x;
                    cur_y = y;
                    segments.push(SvgPathSegment::CubicCurveTo {
                        cp1x,
                        cp1y,
                        cp2x,
                        cp2y,
                        x,
                        y,
                    });
                }
            }
            'Q' => {
                while let (Some(cpx), Some(cpy), Some(x), Some(y)) = (
                    next_num(&mut i),
                    next_num(&mut i),
                    next_num(&mut i),
                    next_num(&mut i),
                ) {
                    cur_x = x;
                    cur_y = y;
                    segments.push(SvgPathSegment::QuadraticCurveTo { cpx, cpy, x, y });
                }
            }
            'q' => {
                while let (Some(dcpx), Some(dcpy), Some(dx), Some(dy)) = (
                    next_num(&mut i),
                    next_num(&mut i),
                    next_num(&mut i),
                    next_num(&mut i),
                ) {
                    let cpx = cur_x + dcpx;
                    let cpy = cur_y + dcpy;
                    let x = cur_x + dx;
                    let y = cur_y + dy;
                    cur_x = x;
                    cur_y = y;
                    segments.push(SvgPathSegment::QuadraticCurveTo { cpx, cpy, x, y });
                }
            }
            'Z' | 'z' => {
                cur_x = start_x;
                cur_y = start_y;
                segments.push(SvgPathSegment::ClosePath);
            }
            _ => {}
        }
    }

    segments
}

pub fn render_svg(svg_node: &Node, target_width: u32, target_height: u32) -> Rc<RasterImage> {
    let mut ctx = CanvasContext2D::new(target_width, target_height);
    let elem = svg_node.as_element();
    let view_box = elem
        .and_then(|e| e.get_attr("viewBox"))
        .and_then(parse_view_box)
        .unwrap_or(SvgViewBox {
            min_x: 0.0,
            min_y: 0.0,
            width: target_width as f32,
            height: target_height as f32,
        });

    let scale_x = if view_box.width > 0.0 {
        target_width as f32 / view_box.width
    } else {
        1.0
    };
    let scale_y = if view_box.height > 0.0 {
        target_height as f32 / view_box.height
    } else {
        1.0
    };

    ctx.save();
    ctx.scale(scale_x, scale_y);
    ctx.translate(-view_box.min_x, -view_box.min_y);

    render_svg_children(svg_node, &mut ctx);

    ctx.restore();
    ctx.to_raster_image()
}

fn render_svg_children(node: &Node, ctx: &mut CanvasContext2D) {
    for child in &node.children {
        let Some(elem) = child.as_element() else {
            continue;
        };
        let tag = elem.tag_name.as_str();

        let fill_color = elem
            .get_attr("fill")
            .and_then(parse_svg_color)
            .unwrap_or(Color::rgb(0, 0, 0));
        let stroke_color = elem
            .get_attr("stroke")
            .and_then(parse_svg_color)
            .unwrap_or(Color::transparent());
        let stroke_width: f32 = elem
            .get_attr("stroke-width")
            .and_then(|s| s.trim_end_matches("px").parse().ok())
            .unwrap_or(1.0);

        ctx.save();
        ctx.fill_style = fill_color;
        ctx.stroke_style = stroke_color;
        ctx.line_width = stroke_width;

        match tag {
            "rect" => {
                let x: f32 = elem
                    .get_attr("x")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let y: f32 = elem
                    .get_attr("y")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let w: f32 = elem
                    .get_attr("width")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let h: f32 = elem
                    .get_attr("height")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                if fill_color.a > 0 {
                    ctx.fill_rect(x, y, w, h);
                }
                if stroke_color.a > 0 && stroke_width > 0.0 {
                    ctx.stroke_rect(x, y, w, h);
                }
            }
            "circle" => {
                let cx: f32 = elem
                    .get_attr("cx")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let cy: f32 = elem
                    .get_attr("cy")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let r: f32 = elem
                    .get_attr("r")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                if r > 0.0 {
                    ctx.begin_path();
                    ctx.arc(cx, cy, r, 0.0, std::f32::consts::TAU, false);
                    if fill_color.a > 0 {
                        ctx.fill();
                    }
                    if stroke_color.a > 0 && stroke_width > 0.0 {
                        ctx.stroke();
                    }
                }
            }
            "line" => {
                let x1: f32 = elem
                    .get_attr("x1")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let y1: f32 = elem
                    .get_attr("y1")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let x2: f32 = elem
                    .get_attr("x2")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let y2: f32 = elem
                    .get_attr("y2")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                if stroke_color.a > 0 && stroke_width > 0.0 {
                    ctx.begin_path();
                    ctx.move_to(x1, y1);
                    ctx.line_to(x2, y2);
                    ctx.stroke();
                }
            }
            "polygon" | "polyline" => {
                if let Some(pts_str) = elem.get_attr("points") {
                    let pts = parse_points(pts_str);
                    if pts.len() >= 2 {
                        ctx.begin_path();
                        ctx.move_to(pts[0].0, pts[0].1);
                        for (px, py) in pts.iter().skip(1) {
                            ctx.line_to(*px, *py);
                        }
                        if tag == "polygon" {
                            ctx.close_path();
                        }
                        if fill_color.a > 0 && tag == "polygon" {
                            ctx.fill();
                        }
                        if stroke_color.a > 0 && stroke_width > 0.0 {
                            ctx.stroke();
                        }
                    }
                }
            }
            "path" => {
                if let Some(d) = elem.get_attr("d") {
                    let segments = parse_svg_path_data(d);
                    if !segments.is_empty() {
                        ctx.begin_path();
                        for seg in &segments {
                            match *seg {
                                SvgPathSegment::MoveTo(x, y) => ctx.move_to(x, y),
                                SvgPathSegment::LineTo(x, y) => ctx.line_to(x, y),
                                SvgPathSegment::CubicCurveTo {
                                    cp1x,
                                    cp1y,
                                    cp2x,
                                    cp2y,
                                    x,
                                    y,
                                } => ctx.bezier_curve_to(cp1x, cp1y, cp2x, cp2y, x, y),
                                SvgPathSegment::QuadraticCurveTo { cpx, cpy, x, y } => {
                                    ctx.quadratic_curve_to(cpx, cpy, x, y)
                                }
                                SvgPathSegment::ClosePath => ctx.close_path(),
                            }
                        }
                        if fill_color.a > 0 {
                            ctx.fill();
                        }
                        if stroke_color.a > 0 && stroke_width > 0.0 {
                            ctx.stroke();
                        }
                    }
                }
            }
            "g" => {
                render_svg_children(child, ctx);
            }
            _ => {}
        }

        ctx.restore();
    }
}
