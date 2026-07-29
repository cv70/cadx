#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds2 {
    pub min: Point2,
    pub max: Point2,
}

impl Bounds2 {
    pub fn from_point(point: Point2) -> Self {
        Self {
            min: point,
            max: point,
        }
    }

    pub fn include_point(&mut self, point: Point2) {
        self.min.x = self.min.x.min(point.x);
        self.min.y = self.min.y.min(point.y);
        self.max.x = self.max.x.max(point.x);
        self.max.y = self.max.y.max(point.y);
    }

    pub fn include_bounds(&mut self, other: Self) {
        self.include_point(other.min);
        self.include_point(other.max);
    }

    pub fn width(self) -> f64 {
        self.max.x - self.min.x
    }

    pub fn height(self) -> f64 {
        self.max.y - self.min.y
    }

    pub fn center(self) -> Point2 {
        Point2::new(
            (self.min.x + self.max.x) * 0.5,
            (self.min.y + self.max.y) * 0.5,
        )
    }

    pub fn contains(self, point: Point2) -> bool {
        point.x >= self.min.x
            && point.x <= self.max.x
            && point.y >= self.min.y
            && point.y <= self.max.y
    }
}
use cadx_core::Point2;
