use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tracing::warn;
use uuid::Uuid;

use crate::protocol::{DrawArrow, DrawCircle, DrawLabel, DrawRect, Message};

pub type SharedShapeStore = Arc<Mutex<ShapeStore>>;
pub type NeedsRedraw = bool;

pub const FADE_MS: u64 = 400;
pub const MAX_SHAPES: usize = 100;

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Rect(DrawRect),
    Circle(DrawCircle),
    Arrow(DrawArrow),
    Label(DrawLabel),
}

impl Shape {
    pub fn from_message(message: Message) -> Option<Self> {
        match message {
            Message::DrawRect(rect) => Some(Self::Rect(rect)),
            Message::DrawCircle(circle) => Some(Self::Circle(circle)),
            Message::DrawArrow(arrow) => Some(Self::Arrow(arrow)),
            Message::DrawLabel(label) => Some(Self::Label(label)),
            Message::Clear(_) => None,
        }
    }

    pub fn id(&self) -> Uuid {
        match self {
            Self::Rect(rect) => rect.style.id,
            Self::Circle(circle) => circle.style.id,
            Self::Arrow(arrow) => arrow.style.id,
            Self::Label(label) => label.style.id,
        }
    }

    pub fn ttl_ms(&self) -> u32 {
        match self {
            Self::Rect(rect) => rect.style.ttl_ms,
            Self::Circle(circle) => circle.style.ttl_ms,
            Self::Arrow(arrow) => arrow.style.ttl_ms,
            Self::Label(label) => label.style.ttl_ms,
        }
    }

    fn set_current_opacity(&mut self, opacity: f64) {
        match self {
            Self::Rect(rect) => rect.style.current_opacity = opacity,
            Self::Circle(circle) => circle.style.current_opacity = opacity,
            Self::Arrow(arrow) => arrow.style.current_opacity = opacity,
            Self::Label(label) => label.style.current_opacity = opacity,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StoredShape {
    pub shape: Shape,
    pub created_at: Instant,
    pub ttl_ms: u32,
    pub current_opacity: f64,
}

#[derive(Debug, Default)]
pub struct ShapeStore {
    shapes: Vec<StoredShape>,
    dirty: bool,
}

impl ShapeStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn shared() -> SharedShapeStore {
        Arc::new(Mutex::new(Self::new()))
    }

    pub fn add(&mut self, shape: Shape) -> Uuid {
        self.add_at(shape, Instant::now())
    }

    pub fn add_at(&mut self, mut shape: Shape, created_at: Instant) -> Uuid {
        self.prune_expired_at(created_at);
        self.evict_oldest_if_full();

        let id = shape.id();
        let ttl_ms = shape.ttl_ms();
        shape.set_current_opacity(1.0);
        self.shapes.push(StoredShape {
            shape,
            created_at,
            ttl_ms,
            current_opacity: 1.0,
        });
        self.dirty = true;
        id
    }

    pub fn remove(&mut self, id: Uuid) -> bool {
        let before = self.shapes.len();
        self.shapes.retain(|shape| shape.shape.id() != id);
        let removed = self.shapes.len() != before;
        self.dirty |= removed;
        removed
    }

    pub fn clear(&mut self) -> NeedsRedraw {
        let needs_redraw = !self.shapes.is_empty();
        self.shapes.clear();
        self.dirty |= needs_redraw;
        needs_redraw
    }

    pub fn len(&self) -> usize {
        self.shapes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }

    pub fn shapes(&self) -> &[StoredShape] {
        &self.shapes
    }

    pub fn snapshot(&self) -> Vec<StoredShape> {
        self.shapes.clone()
    }

    pub fn tick(&mut self) -> NeedsRedraw {
        self.tick_at(Instant::now())
    }

    pub fn tick_at(&mut self, now: Instant) -> NeedsRedraw {
        let mut needs_redraw = std::mem::take(&mut self.dirty);
        needs_redraw |= self.prune_expired_at(now);

        for shape in &mut self.shapes {
            let next_opacity = shape.opacity_at(now);
            if (shape.current_opacity - next_opacity).abs() > f64::EPSILON {
                shape.current_opacity = next_opacity;
                shape.shape.set_current_opacity(next_opacity);
                needs_redraw = true;
            }
        }

        needs_redraw
    }

    fn prune_expired_at(&mut self, now: Instant) -> bool {
        let before = self.shapes.len();
        self.shapes.retain(|shape| !shape.is_expired(now));
        let removed = self.shapes.len() != before;
        self.dirty |= removed;
        removed
    }

    fn evict_oldest_if_full(&mut self) {
        if self.shapes.len() < MAX_SHAPES {
            return;
        }

        if let Some((oldest_index, oldest)) = self
            .shapes
            .iter()
            .enumerate()
            .min_by_key(|(_, shape)| shape.created_at)
        {
            let evicted_id = oldest.shape.id();
            self.shapes.remove(oldest_index);
            self.dirty = true;
            warn!(
                shape_id = %evicted_id,
                max_shapes = MAX_SHAPES,
                "flashpaste-overlayd shape limit reached; evicted oldest shape"
            );
        }
    }
}

impl StoredShape {
    fn expires_at(&self) -> Instant {
        self.created_at + Duration::from_millis(u64::from(self.ttl_ms))
    }

    fn is_expired(&self, now: Instant) -> bool {
        now >= self.expires_at()
    }

    fn opacity_at(&self, now: Instant) -> f64 {
        let elapsed = now.saturating_duration_since(self.created_at);
        let ttl = Duration::from_millis(u64::from(self.ttl_ms));

        if elapsed >= ttl {
            return 0.0;
        }

        let remaining = ttl - elapsed;
        let fade = Duration::from_millis(FADE_MS);
        if remaining >= fade {
            return 1.0;
        }

        remaining.as_secs_f64() / fade.as_secs_f64()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Color, DrawStyle};

    fn style_with_id(id: Uuid, ttl_ms: u32) -> DrawStyle {
        DrawStyle {
            id,
            ttl_ms,
            color: Color(1.0, 0.0, 0.0, 1.0),
            stroke_width: 4.0,
            current_opacity: 1.0,
        }
    }

    fn rect_with_id(id: Uuid, ttl_ms: u32) -> Shape {
        Shape::Rect(DrawRect {
            style: style_with_id(id, ttl_ms),
            x: 10.0,
            y: 20.0,
            w: 30.0,
            h: 40.0,
        })
    }

    fn rect(ttl_ms: u32) -> Shape {
        rect_with_id(Uuid::new_v4(), ttl_ms)
    }

    #[test]
    fn fade_math_stays_full_until_last_400ms() {
        let now = Instant::now();
        let mut store = ShapeStore::new();
        store.add_at(rect(1_000), now);

        assert!(store.tick_at(now));
        assert!(!store.tick_at(now + Duration::from_millis(500)));
        assert_eq!(store.shapes()[0].current_opacity, 1.0);
    }

    #[test]
    fn fade_math_reaches_halfway_at_800ms_for_one_second_ttl() {
        let now = Instant::now();
        let mut store = ShapeStore::new();
        store.add_at(rect(1_000), now);

        assert!(store.tick_at(now + Duration::from_millis(800)));
        let opacity = store.shapes()[0].current_opacity;
        assert!((opacity - 0.5).abs() < 0.01, "opacity was {opacity}");
    }

    #[test]
    fn tick_removes_expired_shapes_and_requests_redraw() {
        let now = Instant::now();
        let mut store = ShapeStore::new();
        store.add_at(rect(1_000), now);

        assert!(store.tick_at(now + Duration::from_millis(1_000)));
        assert!(store.is_empty());
    }

    #[test]
    fn add_evicts_oldest_non_expired_shape_at_limit() {
        let now = Instant::now();
        let mut store = ShapeStore::new();
        let first_id = Uuid::from_u128(1);

        for offset in 0..MAX_SHAPES {
            let id = if offset == 0 {
                first_id
            } else {
                Uuid::from_u128((offset + 1) as u128)
            };
            store.add_at(
                rect_with_id(id, 30_000),
                now + Duration::from_millis(offset as u64),
            );
        }

        let extra_id = Uuid::from_u128(10_000);
        store.add_at(
            rect_with_id(extra_id, 30_000),
            now + Duration::from_millis(MAX_SHAPES as u64),
        );

        assert_eq!(store.len(), MAX_SHAPES);
        assert!(!store
            .shapes()
            .iter()
            .any(|shape| shape.shape.id() == first_id));
        assert!(store
            .shapes()
            .iter()
            .any(|shape| shape.shape.id() == extra_id));
    }

    #[test]
    fn add_prunes_expired_shapes_before_limit_eviction() {
        let now = Instant::now();
        let mut store = ShapeStore::new();
        let expired_id = Uuid::from_u128(20_000);
        store.add_at(rect_with_id(expired_id, 1), now);

        for offset in 0..MAX_SHAPES {
            store.add_at(
                rect_with_id(Uuid::from_u128((30_000 + offset) as u128), 30_000),
                now + Duration::from_millis(2 + offset as u64),
            );
        }

        assert_eq!(store.len(), MAX_SHAPES);
        assert!(!store
            .shapes()
            .iter()
            .any(|shape| shape.shape.id() == expired_id));
    }
}
