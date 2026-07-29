use cadx_core::Point2;

use crate::bounds::Bounds2;
use crate::geometry::finite_point;

pub const MIN_PIXELS_PER_UNIT: f64 = 0.02;
pub const MAX_PIXELS_PER_UNIT: f64 = 10_000.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewportSize {
    pub width: f64,
    pub height: f64,
}

impl ViewportSize {
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }

    pub fn is_valid(self) -> bool {
        self.width.is_finite() && self.height.is_finite() && self.width > 0.0 && self.height > 0.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenPoint {
    pub x: f64,
    pub y: f64,
}

impl ScreenPoint {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewTransform {
    pub center: Point2,
    pub pixels_per_unit: f64,
}

impl Default for ViewTransform {
    fn default() -> Self {
        Self {
            center: Point2::new(0.0, 0.0),
            pixels_per_unit: 3.2,
        }
    }
}

impl ViewTransform {
    pub fn new(center: Point2, pixels_per_unit: f64) -> Self {
        Self {
            center: if finite_point(center) {
                center
            } else {
                Point2::new(0.0, 0.0)
            },
            pixels_per_unit: if pixels_per_unit.is_finite() && pixels_per_unit > 0.0 {
                pixels_per_unit.clamp(MIN_PIXELS_PER_UNIT, MAX_PIXELS_PER_UNIT)
            } else {
                Self::default().pixels_per_unit
            },
        }
    }

    pub fn project(self, point: Point2, viewport: ViewportSize) -> ScreenPoint {
        ScreenPoint::new(
            viewport.width * 0.5 + (point.x - self.center.x) * self.pixels_per_unit,
            viewport.height * 0.5 - (point.y - self.center.y) * self.pixels_per_unit,
        )
    }

    pub fn unproject(self, point: ScreenPoint, viewport: ViewportSize) -> Point2 {
        Point2::new(
            self.center.x + (point.x - viewport.width * 0.5) / self.pixels_per_unit,
            self.center.y - (point.y - viewport.height * 0.5) / self.pixels_per_unit,
        )
    }

    pub fn pan_pixels(&mut self, delta: ScreenPoint) {
        if !delta.x.is_finite() || !delta.y.is_finite() {
            return;
        }
        self.center.x -= delta.x / self.pixels_per_unit;
        self.center.y += delta.y / self.pixels_per_unit;
    }

    /// Zooms while preserving the world coordinate under `anchor`.
    pub fn zoom_at(&mut self, anchor: ScreenPoint, viewport: ViewportSize, factor: f64) -> bool {
        if !viewport.is_valid() || !factor.is_finite() || factor <= 0.0 {
            return false;
        }
        let before = self.unproject(anchor, viewport);
        self.pixels_per_unit =
            (self.pixels_per_unit * factor).clamp(MIN_PIXELS_PER_UNIT, MAX_PIXELS_PER_UNIT);
        let after = self.unproject(anchor, viewport);
        self.center.x += before.x - after.x;
        self.center.y += before.y - after.y;
        true
    }

    pub fn fit_bounds(&mut self, bounds: Bounds2, viewport: ViewportSize, padding: f64) -> bool {
        if !viewport.is_valid() {
            return false;
        }
        let padding = padding.clamp(0.0, 0.45);
        let usable_width = viewport.width * (1.0 - padding * 2.0);
        let usable_height = viewport.height * (1.0 - padding * 2.0);
        let width = bounds.width().max(1.0);
        let height = bounds.height().max(1.0);
        self.center = bounds.center();
        self.pixels_per_unit = (usable_width / width)
            .min(usable_height / height)
            .clamp(MIN_PIXELS_PER_UNIT, MAX_PIXELS_PER_UNIT);
        true
    }
}
