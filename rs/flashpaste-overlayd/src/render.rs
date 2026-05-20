use std::f64::consts::PI;

use anyhow::{Context, Result};
use cairo::{Context as CairoContext, Format, ImageSurface};
use tracing::debug_span;

use crate::{
    protocol::{Color, DrawArrow, DrawCircle, DrawLabel, DrawRect},
    store::{Shape, StoredShape},
};

#[derive(Debug)]
pub struct RenderCtx {
    width: i32,
    height: i32,
    stride: i32,
    surface: ImageSurface,
}

impl RenderCtx {
    pub fn new(width: u32, height: u32) -> Result<Self> {
        let width = width.max(1).min(i32::MAX as u32) as i32;
        let height = height.max(1).min(i32::MAX as u32) as i32;
        let stride = Format::ARgb32
            .stride_for_width(width as u32)
            .context("failed to compute Cairo stride")?;
        let surface = ImageSurface::create(Format::ARgb32, width, height)
            .context("failed to create Cairo image surface")?;

        Ok(Self {
            width,
            height,
            stride,
            surface,
        })
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    pub fn clear_all(&self) -> Result<()> {
        let ctx = CairoContext::new(&self.surface).context("failed to create Cairo context")?;
        ctx.set_operator(cairo::Operator::Clear);
        ctx.paint().context("failed to clear Cairo surface")?;
        ctx.set_operator(cairo::Operator::Over);
        Ok(())
    }

    pub fn draw_shape(&self, shape: &StoredShape) -> Result<()> {
        match &shape.shape {
            Shape::Rect(rect) => self.draw_rect_with_opacity(rect, shape.current_opacity),
            Shape::Circle(circle) => self.draw_circle_with_opacity(circle, shape.current_opacity),
            Shape::Arrow(arrow) => self.draw_arrow_with_opacity(arrow, shape.current_opacity),
            Shape::Label(label) => self.draw_label_with_opacity(label, shape.current_opacity),
        }
    }

    pub fn draw_rect(&self, rect: &DrawRect) -> Result<()> {
        self.draw_rect_with_opacity(rect, rect.style.current_opacity)
    }

    pub fn draw_circle(&self, circle: &DrawCircle) -> Result<()> {
        self.draw_circle_with_opacity(circle, circle.style.current_opacity)
    }

    pub fn draw_arrow(&self, arrow: &DrawArrow) -> Result<()> {
        self.draw_arrow_with_opacity(arrow, arrow.style.current_opacity)
    }

    pub fn draw_label(&self, label: &DrawLabel) -> Result<()> {
        self.draw_label_with_opacity(label, label.style.current_opacity)
    }

    fn draw_rect_with_opacity(&self, rect: &DrawRect, opacity: f64) -> Result<()> {
        let ctx = self.context()?;
        apply_stroke(&ctx, rect.style.color, opacity, rect.style.stroke_width);
        ctx.rectangle(rect.x, rect.y, rect.w.max(0.0), rect.h.max(0.0));
        ctx.stroke().context("failed to stroke rectangle")?;
        Ok(())
    }

    fn draw_circle_with_opacity(&self, circle: &DrawCircle, opacity: f64) -> Result<()> {
        let ctx = self.context()?;
        apply_stroke(&ctx, circle.style.color, opacity, circle.style.stroke_width);

        ctx.save().context("failed to save Cairo state")?;
        ctx.translate(circle.x + circle.w / 2.0, circle.y + circle.h / 2.0);
        ctx.scale(
            (circle.w / 2.0).abs().max(0.1),
            (circle.h / 2.0).abs().max(0.1),
        );
        ctx.arc(0.0, 0.0, 1.0, 0.0, PI * 2.0);
        ctx.restore().context("failed to restore Cairo state")?;
        ctx.stroke().context("failed to stroke circle")?;
        Ok(())
    }

    fn draw_arrow_with_opacity(&self, arrow: &DrawArrow, opacity: f64) -> Result<()> {
        let ctx = self.context()?;
        apply_stroke(&ctx, arrow.style.color, opacity, arrow.style.stroke_width);

        ctx.move_to(arrow.x1, arrow.y1);
        ctx.line_to(arrow.x2, arrow.y2);
        ctx.stroke().context("failed to stroke arrow shaft")?;

        let angle = (arrow.y2 - arrow.y1).atan2(arrow.x2 - arrow.x1);
        let head_len = 12.0;
        let head_angle = 25.0_f64.to_radians();
        let left = angle + PI - head_angle;
        let right = angle + PI + head_angle;
        let left_x = arrow.x2 + head_len * left.cos();
        let left_y = arrow.y2 + head_len * left.sin();
        let right_x = arrow.x2 + head_len * right.cos();
        let right_y = arrow.y2 + head_len * right.sin();

        apply_fill(&ctx, arrow.style.color, opacity);
        ctx.move_to(arrow.x2, arrow.y2);
        ctx.line_to(left_x, left_y);
        ctx.line_to(right_x, right_y);
        ctx.close_path();
        ctx.fill().context("failed to fill arrow head")?;
        Ok(())
    }

    fn draw_label_with_opacity(&self, label: &DrawLabel, opacity: f64) -> Result<()> {
        let ctx = self.context()?;
        ctx.move_to(label.x, label.y);

        ctx.select_font_face(
            "sans-serif",
            cairo::FontSlant::Normal,
            cairo::FontWeight::Normal,
        );
        ctx.set_font_size(14.0);
        ctx.text_path(&label.text);
        apply_stroke(&ctx, label.style.color, opacity, label.style.stroke_width);
        ctx.stroke_preserve().context("failed to stroke label")?;
        apply_fill(&ctx, label.style.color, opacity);
        ctx.fill().context("failed to fill label")?;
        Ok(())
    }

    pub fn render_shapes(&self, shapes: &[StoredShape]) -> Result<()> {
        let _span = debug_span!("render_cairo", shapes = shapes.len()).entered();
        self.clear_all()?;
        for shape in shapes {
            self.draw_shape(shape)?;
        }
        Ok(())
    }

    pub fn copy_to(&mut self, canvas: &mut [u8]) -> Result<()> {
        let _span = debug_span!("copy_to_shm").entered();
        self.surface.flush();
        let stride = self.stride as usize;
        let width_bytes = self.width as usize * 4;
        let height = self.height as usize;
        let data = self
            .surface
            .data()
            .context("failed to borrow Cairo image data")?;

        let required = width_bytes * height;
        anyhow::ensure!(
            canvas.len() >= required,
            "wl_shm canvas too small: {} < {}",
            canvas.len(),
            required
        );

        if stride == width_bytes {
            canvas[..required].copy_from_slice(&data[..required]);
            return Ok(());
        }

        for row in 0..height {
            let src_start = row * stride;
            let dst_start = row * width_bytes;
            canvas[dst_start..dst_start + width_bytes]
                .copy_from_slice(&data[src_start..src_start + width_bytes]);
        }

        Ok(())
    }

    fn context(&self) -> Result<CairoContext> {
        let ctx = CairoContext::new(&self.surface).context("failed to create Cairo context")?;
        ctx.set_antialias(cairo::Antialias::Best);
        Ok(ctx)
    }
}

fn apply_stroke(ctx: &CairoContext, color: Color, opacity: f64, stroke_width: f64) {
    apply_source(ctx, color, opacity);
    ctx.set_line_width(stroke_width.max(0.1));
    ctx.set_line_cap(cairo::LineCap::Round);
    ctx.set_line_join(cairo::LineJoin::Round);
}

fn apply_fill(ctx: &CairoContext, color: Color, opacity: f64) {
    apply_source(ctx, color, opacity);
}

fn apply_source(ctx: &CairoContext, color: Color, opacity: f64) {
    let (r, g, b, a) = color.to_rgba_tuple();
    ctx.set_source_rgba(r, g, b, (a * opacity).clamp(0.0, 1.0));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::DrawStyle;
    use std::time::Instant;
    use uuid::Uuid;

    fn style() -> DrawStyle {
        DrawStyle {
            id: Uuid::new_v4(),
            ttl_ms: 3_000,
            color: Color(1.0, 0.0, 0.0, 1.0),
            stroke_width: 4.0,
            current_opacity: crate::protocol::DEFAULT_CURRENT_OPACITY,
        }
    }

    fn stored(shape: Shape) -> StoredShape {
        StoredShape {
            shape,
            created_at: Instant::now(),
            ttl_ms: 3_000,
            current_opacity: 1.0,
        }
    }

    fn assert_non_empty(ctx: &mut RenderCtx, x: usize, y: usize, w: usize, h: usize) {
        assert!(
            non_empty_pixels(ctx, x, y, w, h) > 0,
            "expected non-empty pixels in target region"
        );
    }

    fn non_empty_pixels(ctx: &mut RenderCtx, x: usize, y: usize, w: usize, h: usize) -> usize {
        ctx.surface.flush();
        let stride = ctx.stride as usize;
        let data = ctx.surface.data().expect("surface data");
        let mut count = 0;

        for row in y..(y + h) {
            for col in x..(x + w) {
                let offset = row * stride + col * 4;
                if data[offset..offset + 4].iter().any(|byte| *byte != 0) {
                    count += 1;
                }
            }
        }

        count
    }

    #[test]
    fn renders_rect_pixels() {
        let mut ctx = RenderCtx::new(128, 128).unwrap();
        ctx.clear_all().unwrap();
        ctx.draw_shape(&stored(Shape::Rect(DrawRect {
            style: style(),
            x: 20.0,
            y: 20.0,
            w: 50.0,
            h: 30.0,
        })))
        .unwrap();
        assert_non_empty(&mut ctx, 18, 18, 56, 36);
    }

    #[test]
    fn renders_circle_pixels() {
        let mut ctx = RenderCtx::new(128, 128).unwrap();
        ctx.clear_all().unwrap();
        ctx.draw_shape(&stored(Shape::Circle(DrawCircle {
            style: style(),
            x: 20.0,
            y: 20.0,
            w: 50.0,
            h: 50.0,
        })))
        .unwrap();
        assert_non_empty(&mut ctx, 20, 20, 54, 54);
    }

    #[test]
    fn renders_arrow_pixels() {
        let mut ctx = RenderCtx::new(128, 128).unwrap();
        ctx.clear_all().unwrap();
        ctx.draw_shape(&stored(Shape::Arrow(DrawArrow {
            style: style(),
            x1: 20.0,
            y1: 20.0,
            x2: 90.0,
            y2: 70.0,
        })))
        .unwrap();
        assert_non_empty(&mut ctx, 80, 60, 24, 24);
    }

    #[test]
    fn renders_label_pixels() {
        let mut ctx = RenderCtx::new(128, 128).unwrap();
        ctx.clear_all().unwrap();
        ctx.draw_shape(&stored(Shape::Label(DrawLabel {
            style: style(),
            x: 20.0,
            y: 20.0,
            text: "fade".to_string(),
        })))
        .unwrap();
        assert_non_empty(&mut ctx, 18, 18, 64, 28);
    }

    #[test]
    fn opacity_multiplies_color_alpha() {
        let mut ctx = RenderCtx::new(128, 128).unwrap();
        ctx.clear_all().unwrap();
        ctx.draw_shape(&StoredShape {
            current_opacity: 0.5,
            ..stored(Shape::Rect(DrawRect {
                style: style(),
                x: 20.0,
                y: 20.0,
                w: 50.0,
                h: 30.0,
            }))
        })
        .unwrap();
        assert_non_empty(&mut ctx, 18, 18, 56, 36);
    }

    #[test]
    fn direct_draw_uses_shape_current_opacity() {
        let mut ctx = RenderCtx::new(128, 128).unwrap();
        ctx.clear_all().unwrap();
        let mut rect_style = style();
        rect_style.current_opacity = 0.0;

        ctx.draw_rect(&DrawRect {
            style: rect_style,
            x: 20.0,
            y: 20.0,
            w: 50.0,
            h: 30.0,
        })
        .unwrap();

        assert_eq!(non_empty_pixels(&mut ctx, 18, 18, 56, 36), 0);
    }
}
