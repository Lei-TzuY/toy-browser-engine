// ============================================================
//  canvas.rs  —  HTML5 <canvas> 2D Rendering Context
// ============================================================
//
//  Provides software rasterization for 2D canvas drawing operations:
//   • Rectangles (fillRect, strokeRect, clearRect)
//   • Paths (beginPath, closePath, moveTo, lineTo, arc, rect)
//   • Path rasterization (fill with scanline/triangulation, stroke with line width)
//   • Text rendering (fillText using fontdue glyph rasterization)
//   • State stack (save, restore for styles, alpha, line width, font)
//   • Pixel manipulation (getImageData, putImageData)
//   • Conversion to RasterImage for seamless paint integration

use std::rc::Rc;

use crate::css::parser::Color;
use crate::image::RasterImage;
use crate::layout::TextAlign;
use crate::text::{measure_text, rasterize};

#[derive(Debug, Clone)]
struct CanvasState {
    fill_style: Color,
    stroke_style: Color,
    line_width: f32,
    font_size: f32,
    text_align: TextAlign,
    global_alpha: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PathCommand {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    Arc {
        cx: f32,
        cy: f32,
        radius: f32,
        start_angle: f32,
        end_angle: f32,
        counterclockwise: bool,
    },
    ClosePath,
}

#[derive(Debug, Clone)]
pub struct CanvasContext2D {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub fill_style: Color,
    pub stroke_style: Color,
    pub line_width: f32,
    pub font_size: f32,
    pub text_align: TextAlign,
    pub global_alpha: f32,
    state_stack: Vec<CanvasState>,
    path: Vec<PathCommand>,
    current_point: (f32, f32),
}

impl CanvasContext2D {
    pub fn new(width: u32, height: u32) -> Self {
        let w = width.max(1);
        let h = height.max(1);
        let pixels = vec![0u8; (w * h * 4) as usize];
        Self {
            width: w,
            height: h,
            pixels,
            fill_style: Color::rgb(0, 0, 0),
            stroke_style: Color::rgb(0, 0, 0),
            line_width: 1.0,
            font_size: 10.0,
            text_align: TextAlign::Left,
            global_alpha: 1.0,
            state_stack: Vec::new(),
            path: Vec::new(),
            current_point: (0.0, 0.0),
        }
    }

    pub fn to_raster_image(&self) -> Rc<RasterImage> {
        Rc::new(RasterImage::new(self.width, self.height, self.pixels.clone()))
    }

    pub fn save(&mut self) {
        self.state_stack.push(CanvasState {
            fill_style: self.fill_style,
            stroke_style: self.stroke_style,
            line_width: self.line_width,
            font_size: self.font_size,
            text_align: self.text_align,
            global_alpha: self.global_alpha,
        });
    }

    pub fn restore(&mut self) {
        if let Some(state) = self.state_stack.pop() {
            self.fill_style = state.fill_style;
            self.stroke_style = state.stroke_style;
            self.line_width = state.line_width;
            self.font_size = state.font_size;
            self.text_align = state.text_align;
            self.global_alpha = state.global_alpha;
        }
    }

    // ── Pixel manipulation helpers ────────────────────────────────────────────

    #[inline]
    fn blend_pixel(&mut self, x: i32, y: i32, color: Color) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let alpha = (color.a as f32 / 255.0) * self.global_alpha;
        if alpha <= 0.0 {
            return;
        }
        let idx = ((y as usize * self.width as usize) + x as usize) * 4;
        if idx + 3 >= self.pixels.len() {
            return;
        }

        if alpha >= 1.0 {
            self.pixels[idx] = color.r;
            self.pixels[idx + 1] = color.g;
            self.pixels[idx + 2] = color.b;
            self.pixels[idx + 3] = 255;
        } else {
            let src_r = color.r as f32;
            let src_g = color.g as f32;
            let src_b = color.b as f32;

            let dst_r = self.pixels[idx] as f32;
            let dst_g = self.pixels[idx + 1] as f32;
            let dst_b = self.pixels[idx + 2] as f32;
            let dst_a = self.pixels[idx + 3] as f32 / 255.0;

            let out_a = alpha + dst_a * (1.0 - alpha);
            if out_a > 0.0 {
                let out_r = (src_r * alpha + dst_r * dst_a * (1.0 - alpha)) / out_a;
                let out_g = (src_g * alpha + dst_g * dst_a * (1.0 - alpha)) / out_a;
                let out_b = (src_b * alpha + dst_b * dst_a * (1.0 - alpha)) / out_a;

                self.pixels[idx] = out_r.round() as u8;
                self.pixels[idx + 1] = out_g.round() as u8;
                self.pixels[idx + 2] = out_b.round() as u8;
                self.pixels[idx + 3] = (out_a * 255.0).round() as u8;
            }
        }
    }

    #[inline]
    fn set_pixel_exact(&mut self, x: i32, y: i32, r: u8, g: u8, b: u8, a: u8) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let idx = ((y as usize * self.width as usize) + x as usize) * 4;
        if idx + 3 < self.pixels.len() {
            self.pixels[idx] = r;
            self.pixels[idx + 1] = g;
            self.pixels[idx + 2] = b;
            self.pixels[idx + 3] = a;
        }
    }

    // ── Rectangles ────────────────────────────────────────────────────────────

    pub fn clear_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let x0 = (x.min(x + w)).floor().max(0.0) as u32;
        let y0 = (y.min(y + h)).floor().max(0.0) as u32;
        let x1 = (x.max(x + w)).ceil().min(self.width as f32) as u32;
        let y1 = (y.max(y + h)).ceil().min(self.height as f32) as u32;

        for cy in y0..y1 {
            for cx in x0..x1 {
                self.set_pixel_exact(cx as i32, cy as i32, 0, 0, 0, 0);
            }
        }
    }

    pub fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let x0 = (x.min(x + w)).floor().max(0.0) as u32;
        let y0 = (y.min(y + h)).floor().max(0.0) as u32;
        let x1 = (x.max(x + w)).ceil().min(self.width as f32) as u32;
        let y1 = (y.max(y + h)).ceil().min(self.height as f32) as u32;

        let color = self.fill_style;
        for cy in y0..y1 {
            for cx in x0..x1 {
                self.blend_pixel(cx as i32, cy as i32, color);
            }
        }
    }

    pub fn stroke_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let lw = self.line_width;
        let half = lw / 2.0;
        let color = self.stroke_style;

        // Top, bottom, left, right edges
        let x0 = x - half;
        let y0 = y - half;

        self.fill_rect_color(x0, y0, w + lw, lw, color);
        self.fill_rect_color(x0, y + h - half, w + lw, lw, color);
        self.fill_rect_color(x0, y0, lw, h + lw, color);
        self.fill_rect_color(x + w - half, y0, lw, h + lw, color);
    }

    fn fill_rect_color(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color) {
        let x0 = (x.min(x + w)).floor().max(0.0) as u32;
        let y0 = (y.min(y + h)).floor().max(0.0) as u32;
        let x1 = (x.max(x + w)).ceil().min(self.width as f32) as u32;
        let y1 = (y.max(y + h)).ceil().min(self.height as f32) as u32;

        for cy in y0..y1 {
            for cx in x0..x1 {
                self.blend_pixel(cx as i32, cy as i32, color);
            }
        }
    }

    // ── Paths ─────────────────────────────────────────────────────────────────

    pub fn begin_path(&mut self) {
        self.path.clear();
        self.current_point = (0.0, 0.0);
    }

    pub fn close_path(&mut self) {
        self.path.push(PathCommand::ClosePath);
    }

    pub fn move_to(&mut self, x: f32, y: f32) {
        self.current_point = (x, y);
        self.path.push(PathCommand::MoveTo(x, y));
    }

    pub fn line_to(&mut self, x: f32, y: f32) {
        self.current_point = (x, y);
        self.path.push(PathCommand::LineTo(x, y));
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.move_to(x, y);
        self.line_to(x + w, y);
        self.line_to(x + w, y + h);
        self.line_to(x, y + h);
        self.close_path();
    }

    pub fn arc(
        &mut self,
        cx: f32,
        cy: f32,
        radius: f32,
        start_angle: f32,
        end_angle: f32,
        counterclockwise: bool,
    ) {
        self.path.push(PathCommand::Arc {
            cx,
            cy,
            radius,
            start_angle,
            end_angle,
            counterclockwise,
        });
        // Update current point to arc end
        let end_x = cx + radius * end_angle.cos();
        let end_y = cy + radius * end_angle.sin();
        self.current_point = (end_x, end_y);
    }

    /// Flatten path commands into lists of connected line segments / polygons.
    fn flatten_path(&self) -> Vec<Vec<(f32, f32)>> {
        let mut subpaths: Vec<Vec<(f32, f32)>> = Vec::new();
        let mut current_subpath: Vec<(f32, f32)> = Vec::new();

        for cmd in &self.path {
            match cmd {
                PathCommand::MoveTo(x, y) => {
                    if !current_subpath.is_empty() {
                        subpaths.push(current_subpath);
                        current_subpath = Vec::new();
                    }
                    current_subpath.push((*x, *y));
                }
                PathCommand::LineTo(x, y) => {
                    if current_subpath.is_empty() {
                        current_subpath.push((*x, *y));
                    } else {
                        current_subpath.push((*x, *y));
                    }
                }
                PathCommand::Arc {
                    cx,
                    cy,
                    radius,
                    start_angle,
                    end_angle,
                    counterclockwise,
                } => {
                    let mut start = *start_angle;
                    let mut end = *end_angle;
                    let two_pi = std::f32::consts::PI * 2.0;

                    if *counterclockwise {
                        if start <= end {
                            start += two_pi;
                        }
                    } else if end <= start {
                        end += two_pi;
                    }

                    let diff = (end - start).abs();
                    let steps = (diff * radius / 3.0).ceil().max(12.0) as usize;
                    let step_angle = (end - start) / steps as f32;

                    for i in 0..=steps {
                        let theta = start + step_angle * i as f32;
                        let px = cx + radius * theta.cos();
                        let py = cy + radius * theta.sin();
                        current_subpath.push((px, py));
                    }
                }
                PathCommand::ClosePath => {
                    if let Some(&first) = current_subpath.first() {
                        current_subpath.push(first);
                    }
                    if !current_subpath.is_empty() {
                        subpaths.push(current_subpath);
                        current_subpath = Vec::new();
                    }
                }
            }
        }
        if !current_subpath.is_empty() {
            subpaths.push(current_subpath);
        }
        subpaths
    }

    pub fn fill(&mut self) {
        let subpaths = self.flatten_path();
        let color = self.fill_style;
        let h = self.height as i32;
        let w = self.width as i32;

        for subpath in &subpaths {
            if subpath.len() < 3 {
                continue;
            }
            let min_y = subpath
                .iter()
                .map(|p| p.1)
                .fold(f32::INFINITY, f32::min)
                .floor() as i32;
            let max_y = subpath
                .iter()
                .map(|p| p.1)
                .fold(f32::NEG_INFINITY, f32::max)
                .ceil() as i32;

            let y_start = min_y.max(0);
            let y_end = max_y.min(h);

            for scan_y in y_start..y_end {
                let y = scan_y as f32 + 0.5;
                let mut intersections: Vec<f32> = Vec::new();

                for i in 0..subpath.len() {
                    let p0 = subpath[i];
                    let p1 = subpath[(i + 1) % subpath.len()];

                    if (p0.1 <= y && p1.1 > y) || (p1.1 <= y && p0.1 > y) {
                        let t = (y - p0.1) / (p1.1 - p0.1);
                        let x = p0.0 + t * (p1.0 - p0.0);
                        intersections.push(x);
                    }
                }

                intersections.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

                for chunk in intersections.chunks(2) {
                    if chunk.len() == 2 {
                        let x0 = chunk[0].floor().max(0.0) as i32;
                        let x1 = chunk[1].ceil().min(w as f32) as i32;
                        for x in x0..x1 {
                            self.blend_pixel(x, scan_y, color);
                        }
                    }
                }
            }
        }
    }

    pub fn stroke(&mut self) {
        let subpaths = self.flatten_path();
        let color = self.stroke_style;
        let lw = self.line_width.max(1.0);

        for subpath in &subpaths {
            if subpath.len() < 2 {
                continue;
            }
            for i in 0..(subpath.len() - 1) {
                let p0 = subpath[i];
                let p1 = subpath[i + 1];
                self.draw_line(p0.0, p0.1, p1.0, p1.1, lw, color);
            }
        }
    }

    fn draw_line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, color: Color) {
        let dx = x1 - x0;
        let dy = y1 - y0;
        let dist = (dx * dx + dy * dy).sqrt();
        if dist < 0.001 {
            self.draw_disc(x0, y0, width / 2.0, color);
            return;
        }

        let steps = (dist * 2.0).ceil() as usize;
        let half_w = width / 2.0;
        for s in 0..=steps {
            let t = s as f32 / steps as f32;
            let cx = x0 + t * dx;
            let cy = y0 + t * dy;
            self.draw_disc(cx, cy, half_w, color);
        }
    }

    fn draw_disc(&mut self, cx: f32, cy: f32, radius: f32, color: Color) {
        let r = radius.max(0.5);
        let x0 = (cx - r).floor().max(0.0) as i32;
        let x1 = (cx + r).ceil().min(self.width as f32) as i32;
        let y0 = (cy - r).floor().max(0.0) as i32;
        let y1 = (cy + r).ceil().min(self.height as f32) as i32;
        let r2 = r * r;

        for y in y0..y1 {
            for x in x0..x1 {
                let dist2 = (x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2);
                if dist2 <= r2 {
                    self.blend_pixel(x, y, color);
                }
            }
        }
    }

    // ── Text rendering ────────────────────────────────────────────────────────

    pub fn fill_text(&mut self, text: &str, mut x: f32, y: f32) {
        let font_size = self.font_size;
        let measured = measure_text(text, font_size);
        match self.text_align {
            TextAlign::Center => x -= measured / 2.0,
            TextAlign::Right => x -= measured,
            TextAlign::Left => {}
        }

        let color = self.fill_style;
        let mut cursor_x = x;

        for c in text.chars() {
            if let Some((metrics, bitmap)) = rasterize(c, font_size) {
                let gx = cursor_x + metrics.xmin as f32;
                let gy = y - metrics.height as f32 - metrics.ymin as f32;

                for row in 0..metrics.height {
                    for col in 0..metrics.width {
                        let coverage = bitmap[row * metrics.width + col];
                        if coverage > 0 {
                            let mut px_color = color;
                            px_color.a = ((color.a as f32 * (coverage as f32 / 255.0)).round()
                                as u8)
                                .max(1);
                            self.blend_pixel(
                                (gx + col as f32).round() as i32,
                                (gy + row as f32).round() as i32,
                                px_color,
                            );
                        }
                    }
                }
                cursor_x += metrics.advance_width;
            } else {
                cursor_x += font_size * 0.6;
            }
        }
    }

    // ── Image Data ────────────────────────────────────────────────────────────

    pub fn get_image_data(&self, sx: i32, sy: i32, sw: u32, sh: u32) -> Vec<u8> {
        let mut out = vec![0u8; (sw * sh * 4) as usize];
        for y in 0..sh {
            for x in 0..sw {
                let src_x = sx + x as i32;
                let src_y = sy + y as i32;
                let out_idx = ((y * sw + x) * 4) as usize;
                if src_x >= 0
                    && src_y >= 0
                    && src_x < self.width as i32
                    && src_y < self.height as i32
                {
                    let in_idx = ((src_y as usize * self.width as usize) + src_x as usize) * 4;
                    out[out_idx..out_idx + 4].copy_from_slice(&self.pixels[in_idx..in_idx + 4]);
                }
            }
        }
        out
    }

    pub fn put_image_data(&mut self, data: &[u8], dx: i32, dy: i32, sw: u32, sh: u32) {
        for y in 0..sh {
            for x in 0..sw {
                let in_idx = ((y * sw + x) * 4) as usize;
                if in_idx + 3 < data.len() {
                    let r = data[in_idx];
                    let g = data[in_idx + 1];
                    let b = data[in_idx + 2];
                    let a = data[in_idx + 3];
                    self.set_pixel_exact(dx + x as i32, dy + y as i32, r, g, b, a);
                }
            }
        }
    }
}
