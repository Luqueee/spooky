//! Toolkit-neutral geometry, in logical pixels with the origin at the top left.
//!
//! The configuration, split, placement and platform code reason about points, sizes
//! and rectangles without depending on a GUI toolkit; the view layer converts at its
//! boundary with `From`.

/// A position.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub(crate) const ORIGIN: Self = Self::new(0.0, 0.0);

    pub(crate) const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// A width and a height.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct Size {
    pub width: f32,
    pub height: f32,
}

impl Size {
    pub(crate) const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

/// An axis-aligned rectangle given by its top-left corner and its size.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct Rectangle {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rectangle {
    pub(crate) const fn new(origin: Point, size: Size) -> Self {
        Self {
            x: origin.x,
            y: origin.y,
            width: size.width,
            height: size.height,
        }
    }

    /// The smallest rectangle holding both.
    pub(crate) fn union(&self, other: &Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = (self.x + self.width).max(other.x + other.width);
        let bottom = (self.y + self.height).max(other.y + other.height);
        Self {
            x,
            y,
            width: right - x,
            height: bottom - y,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn union_spans_both_rectangles() {
        let a = Rectangle::new(Point::new(10.0, 20.0), Size::new(100.0, 50.0));
        let b = Rectangle::new(Point::new(0.0, 60.0), Size::new(40.0, 100.0));
        assert_eq!(
            a.union(&b),
            Rectangle {
                x: 0.0,
                y: 20.0,
                width: 110.0,
                height: 140.0
            }
        );
        assert_eq!(a.union(&b), b.union(&a));
    }
}
